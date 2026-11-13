//! Freeze / unfreeze: authority ordering (D-50), the atomic state + effects + audit transaction,
//! and the helpers the other domains use to refuse work for a frozen agent (docs/04 §Freeze).

use super::dto::{FreezeResult, Frozen};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::agents::dto::Agent;
use crate::domains::conversations::service as conv;
use crate::domains::hosted::lifecycle;
use aurora_core::{ApiError, FieldError, sanitize};
use aurora_db::DbError;
use aurora_db::repo::conversations as convs;
use aurora_db::repo::credentials;
use aurora_db::repo::freeze::{self as repo, Effective, Scope};
use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

pub const REASON_MAX: usize = 500;
/// Text of the `notice` appended to a conversation whose turn the freeze ended.
const ABORT_NOTICE: &str = "The agent was paused by an administrator; this turn was stopped.";
/// Bound on the turns ended per round (the loop repeats until none are left).
const ABORT_ROUND: i64 = 500;

/// Who is freezing or unfreezing, and the highest authority they act with on this route.
pub struct Freezer {
    pub user_id: Uuid,
    pub actor: AuditActor,
    /// `workspace_owner` | `org_admin` | `instance_admin`.
    pub authority: &'static str,
}

/// Authority a workspace owner acts with: org owners/admins rank above a plain workspace owner.
pub fn workspace_authority(org_admin: bool) -> &'static str {
    if org_admin {
        "org_admin"
    } else {
        "workspace_owner"
    }
}

/// 423 `frozen`: the agent is paused (`level` = where the flag that stops it is set).
pub fn frozen_error(level: Scope) -> ApiError {
    ApiError::new(
        StatusCode::LOCKED,
        "frozen",
        "This agent is paused by an administrator.",
    )
    .with_details(json!({ "level": level.as_str() }))
}

/// 423 `frozen` when the agent is effectively frozen. Called by every operation that would start
/// agent work (messages, attachments, send-now, run-now, peer messaging, BYO endpoints).
pub async fn ensure_agent_active(conn: &mut PgConnection, agent: Uuid) -> Result<(), ApiError> {
    match repo::effective_of_agent(conn, agent).await? {
        Some(e) => Err(frozen_error(e.level)),
        None => Ok(()),
    }
}

/// Last "paused" reply per (integration, space, thread, person); in-process, so a replica
/// replies at most once per interval and the worst case across replicas is one per replica.
type ReplyKey = (Uuid, String, String, String);
type Replies = std::sync::Mutex<std::collections::HashMap<ReplyKey, std::time::Instant>>;
static REPLIES: std::sync::OnceLock<Replies> = std::sync::OnceLock::new();

/// Whether a "paused by an administrator" reply should go out now for this (space/thread,
/// person) (`FREEZE_REPLY_INTERVAL`, docs/13 §5.1 step 2a). Records the reply when it returns `true`.
pub fn should_reply_paused(integration: Uuid, space: &str, thread: &str, person: &str) -> bool {
    let map = REPLIES.get_or_init(Default::default);
    let Ok(mut m) = map.lock() else { return true };
    let now = std::time::Instant::now();
    let interval = aurora_core::limits::FREEZE_REPLY_INTERVAL;
    if m.len() > 10_000 {
        m.retain(|_, t| now.duration_since(*t) < interval);
    }
    let key = (
        integration,
        space.to_owned(),
        thread.to_owned(),
        person.to_owned(),
    );
    match m.get(&key) {
        Some(t) if now.duration_since(*t) < interval => false,
        _ => {
            m.insert(key, now);
            true
        }
    }
}

/// [`ensure_agent_active`] on a pooled connection (for handlers outside a transaction).
pub async fn ensure_agent_active_pool(state: &AppState, agent: Uuid) -> Result<(), ApiError> {
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    ensure_agent_active(&mut conn, agent).await
}

/// The reason, sanitized to one line, 1..=500 characters.
pub fn validate_reason(raw: &str) -> Result<String, ApiError> {
    let bad = |m: &str| {
        ApiError::invalid_request("Invalid request.", vec![FieldError::new("/reason", m)])
    };
    if raw.chars().count() > REASON_MAX {
        return Err(bad("must be at most 500 characters"));
    }
    let s = sanitize::one_line(raw, REASON_MAX);
    let s = s.trim();
    if s.is_empty() {
        return Err(bad("is required"));
    }
    Ok(s.to_owned())
}

fn insufficient(have: &str, need: &str) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "freeze_authority_insufficient",
        "This was frozen by a higher authority; only that authority (or a higher one) can lift it.",
    )
    .with_details(json!({ "required_authority": need, "caller_authority": have }))
}

fn label(scope: Scope) -> &'static str {
    scope.as_str()
}

/// What a freeze covers, resolved from the database by the route.
pub struct Target {
    pub scope: Scope,
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
}

/// Freeze `target`. Idempotent: freezing a frozen scope keeps the original record and answers
/// 200 without a second audit row. State, effects and audit commit together.
pub async fn freeze(
    state: &AppState,
    target: &Target,
    who: &Freezer,
    reason: &str,
) -> Result<FreezeResult, ApiError> {
    let reason = validate_reason(reason)?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    if !repo::lock(&mut tx, target.scope, target.id).await? {
        return Err(ApiError::not_found("Not found."));
    }
    // (FIX-109, D-50) Freezing a scope that a lower authority already froze raises the record to
    // the caller's authority (who, reason, authority; audited with `escalated`): otherwise the
    // lower authority could still lift what the higher one meant to hold. An equal or higher
    // stored authority is left alone and the call says `unchanged`.
    let existing = repo::get(&mut tx, target.scope, target.id, false).await?;
    let escalated_from = match &existing {
        Some(cur) if repo::authority_rank(who.authority) > repo::authority_rank(&cur.authority) => {
            Some(cur.authority.clone())
        }
        _ => None,
    };
    let created = existing.is_none()
        && repo::freeze(
            &mut tx,
            target.scope,
            target.id,
            who.user_id,
            &reason,
            who.authority,
        )
        .await?;
    let escalated = escalated_from.is_some()
        && repo::escalate(
            &mut tx,
            target.scope,
            target.id,
            who.user_id,
            &reason,
            who.authority,
        )
        .await?;
    let outcome = if created {
        "frozen"
    } else if escalated {
        "escalated"
    } else {
        "unchanged"
    };
    let mut flush = Vec::new();
    if escalated {
        let mut ev = AuditEvent::new(
            who.actor.clone(),
            format!("{}.freeze", label(target.scope)),
            Outcome::Success,
        )
        .org(target.org_id)
        .target(label(target.scope), target.id)
        .metadata(json!({
            "reason": reason,
            "authority": who.authority,
            "escalated": true,
            "previous_authority": escalated_from,
        }));
        if let Some(ws) = target.workspace_id {
            ev = ev.workspace(ws);
        }
        audit::insert_tx(&mut tx, &ev).await.map_err(audit_failed)?;
    }
    if created {
        flush = apply_effects(&mut tx, target).await?;
        // (FIX-042) The next home sync must carry an empty `previews` part (the allow-list dies
        // with the sync, `11 §Port previews`): bump the affected agents' generation inside the
        // same transaction so the sync is not left to an unrelated change.
        bump_agents_of(&mut tx, target).await?;
        let mut ev = AuditEvent::new(
            who.actor.clone(),
            format!("{}.freeze", label(target.scope)),
            Outcome::Success,
        )
        .org(target.org_id)
        .target(label(target.scope), target.id)
        .metadata(json!({ "reason": reason, "authority": who.authority }));
        if let Some(ws) = target.workspace_id {
            ev = ev.workspace(ws);
        }
        audit::insert_tx(&mut tx, &ev).await.map_err(audit_failed)?;
    }
    credentials::notify_gateway_invalidate(&mut *tx, label(target.scope), target.id).await?;
    let own = repo::get(&mut tx, target.scope, target.id, false).await?;
    tx.commit().await.map_err(DbError::from)?;
    for (c, t) in flush {
        lifecycle::flush_deltas(state, c, t).await;
    }
    Ok(FreezeResult {
        frozen: own.map(|f| {
            Frozen::from(&Effective {
                level: target.scope,
                frozen: f,
            })
        }),
        effective_frozen: true,
        outcome: Some(outcome.to_owned()),
    })
}

/// Unfreeze `target`: needs at least the authority that froze it (`freeze_authority_insufficient`
/// otherwise). Idempotent on an unfrozen scope. Credentials were never revoked, so service
/// resumes at the next gateway cache refresh; aborted turns and abandoned approvals stay ended.
pub async fn unfreeze(
    state: &AppState,
    target: &Target,
    who: &Freezer,
) -> Result<FreezeResult, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    if !repo::lock(&mut tx, target.scope, target.id).await? {
        return Err(ApiError::not_found("Not found."));
    }
    if let Some(cur) = repo::get(&mut tx, target.scope, target.id, false).await? {
        if repo::authority_rank(who.authority) < repo::authority_rank(&cur.authority) {
            return Err(insufficient(who.authority, &cur.authority));
        }
        repo::unfreeze(&mut tx, target.scope, target.id).await?;
        // (FIX-042) Unfreeze bumps too so access restoration is not delayed: the next home sync
        // re-carries the share list inside the same transaction.
        bump_agents_of(&mut tx, target).await?;
        let mut ev = AuditEvent::new(
            who.actor.clone(),
            format!("{}.unfreeze", label(target.scope)),
            Outcome::Success,
        )
        .org(target.org_id)
        .target(label(target.scope), target.id)
        .metadata(json!({
            "authority": who.authority,
            "frozen_authority": cur.authority,
            "frozen_reason": cur.reason,
        }));
        if let Some(ws) = target.workspace_id {
            ev = ev.workspace(ws);
        }
        audit::insert_tx(&mut tx, &ev).await.map_err(audit_failed)?;
        credentials::notify_gateway_invalidate(&mut *tx, label(target.scope), target.id).await?;
        // A turn queued meanwhile may now be dispatched.
        conv::notify_work(&mut tx).await?;
    }
    let effective_frozen = match target.scope {
        Scope::Agent => repo::effective_of_agent(&mut tx, target.id)
            .await?
            .is_some(),
        Scope::Workspace => repo::effective_of_workspace(&mut tx, target.id)
            .await?
            .is_some(),
        Scope::Org => repo::get(&mut tx, Scope::Org, target.id, false)
            .await?
            .is_some(),
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(FreezeResult {
        frozen: None,
        effective_frozen,
        outcome: None,
    })
}

fn audit_failed(e: DbError) -> ApiError {
    tracing::error!(error = %e, "freeze audit write failed");
    ApiError::unavailable("audit_unavailable", "The audit log is unavailable.")
}

/// (FIX-042) Bumps the home-sync generation of every agent the freeze state change touches
/// (the `bump_org` pattern): the next sync re-renders the `previews` part — empty after a
/// freeze, the share list again after an unfreeze — without waiting for an unrelated change.
async fn bump_agents_of(tx: &mut PgConnection, target: &Target) -> Result<(), DbError> {
    let agents = repo::agents_in(tx, target.scope, target.id).await?;
    if agents.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE agents SET home_sync_generation = home_sync_generation + 1 WHERE id = ANY($1)",
    )
    .bind(&agents)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Effects of a new freeze that are database state (docs/04 §Freeze effects 1–3): pending
/// approvals end, queued/joining/dispatched/running turns end `aborted` with `error_code =
/// frozen`. The sandbox is parked by due-work arm 2 and the gateway reacts to the invalidation the
/// caller emits. Returns `(conversation, turn)` whose buffered deltas must be flushed after commit.
async fn apply_effects(
    tx: &mut PgConnection,
    target: &Target,
) -> Result<Vec<(Uuid, Uuid)>, ApiError> {
    let agents = repo::agents_in(tx, target.scope, target.id).await?;
    if agents.is_empty() {
        return Ok(Vec::new());
    }
    for (id, ws) in repo::abandon_pending_approvals(tx, &agents).await? {
        aurora_db::bus::notify(
            &mut *tx,
            aurora_db::bus::Channel::Approval,
            // (FIX-109) A freeze-abandoned approval is not an expired one: the dashboard shows
            // its own copy for the `abandoned` event (docs/05 SSE `approval` events).
            &json!({"id": id, "workspace_id": ws, "event": "abandoned"}),
        )
        .await
        .map_err(|e| DbError::Other(sqlx::Error::Protocol(e.to_string())))?;
    }
    repo::reject_pending_action_approvals(tx, &agents).await?;
    let mut flush = Vec::new();
    // Ending a turn promotes its `joining` follow-ups into queued turns; repeat until none remain.
    for _ in 0..50 {
        let turns = repo::active_turns_of(tx, &agents, ABORT_ROUND).await?;
        if turns.is_empty() {
            break;
        }
        let ended = end_turns(tx, turns, &mut flush).await?;
        if ended == 0 {
            break;
        }
    }
    Ok(flush)
}

/// End the given active turns `aborted` / `frozen`. Returns how many ended.
async fn end_turns(
    tx: &mut PgConnection,
    turns: Vec<(Uuid, Uuid)>,
    flush: &mut Vec<(Uuid, Uuid)>,
) -> Result<usize, ApiError> {
    let mut n = 0;
    for (turn, conversation) in turns {
        convs::lock(&mut *tx, conversation).await?;
        conv::append_event(
            tx,
            conversation,
            Some(turn),
            "notice",
            &json!({"code": "frozen", "text": ABORT_NOTICE}),
        )
        .await?;
        if lifecycle::finish(
            tx,
            turn,
            "aborted",
            Some("frozen"),
            Some("Paused by an administrator."),
            None,
        )
        .await?
        .is_some()
        {
            n += 1;
            flush.push((conversation, turn));
        }
    }
    Ok(n)
}

/// Sweep (due-work): active turns of effectively frozen agents — a turn created in a race with
/// the freeze — end `aborted` / `frozen`. Bounded per poll.
pub async fn sweep_frozen_turns(
    tx: &mut PgConnection,
    flush: &mut Vec<(Uuid, Uuid)>,
) -> Result<(), ApiError> {
    let turns = repo::active_turns_of_frozen(tx, 16).await?;
    if !turns.is_empty() {
        end_turns(tx, turns, flush).await?;
    }
    Ok(())
}

/// Freeze records of workspaces (own flag or the organization's), keyed by workspace id.
pub async fn workspace_frozen(
    state: &AppState,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Frozen>, ApiError> {
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    Ok(repo::effective_of_workspaces(&mut conn, ids)
        .await?
        .into_iter()
        .map(|(k, e)| (k, Frozen::from(&e)))
        .collect())
}

/// Freeze records of organizations, keyed by org id.
pub async fn org_frozen(
    state: &AppState,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Frozen>, ApiError> {
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    Ok(repo::frozen_of_orgs(&mut conn, ids)
        .await?
        .into_iter()
        .map(|(k, f)| {
            (
                k,
                Frozen::from(&Effective {
                    level: Scope::Org,
                    frozen: f,
                }),
            )
        })
        .collect())
}

/// Fill `frozen` / `effective_frozen` of agent DTOs (one query for the whole slice).
pub async fn decorate_agents(state: &AppState, agents: &mut [Agent]) -> Result<(), ApiError> {
    if agents.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = agents.iter().map(|a| a.id).collect();
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    let map = repo::effective_of_agents(&mut conn, &ids).await?;
    for a in agents {
        if let Some(e) = map.get(&a.id) {
            a.frozen = Some(Frozen::from(e));
            a.effective_frozen = true;
        }
    }
    Ok(())
}

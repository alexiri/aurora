//! Human side of gateway approval holds (docs/07 §Approvals, docs/05 §Approvals).
//! The gateway inserts `pending` rows (`repo::gateway`); this module lists and
//! resolves them and runs the expiry/abandon sweeps. Every transition emits
//! `aurora_approval` in the same transaction (delivered on commit).

use super::{After, split};
use crate::DbError;
use crate::bus::{self, Channel};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// An approval joined with agent and decider labels.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApprovalRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub agent_name: Option<String>,
    pub agent_handle: Option<String>,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub method: String,
    pub host: String,
    pub path: String,
    pub app_id: Option<String>,
    pub tool_id: Option<String>,
    pub connection_id: Option<Uuid>,
    pub rule_logical_id: Option<Uuid>,
    pub rule_name: Option<String>,
    pub summary: serde_json::Value,
    pub batch_id: Option<String>,
    pub batch_label: Option<String>,
    /// (RFE-025) label of the BYO context credential that made the request.
    pub context_label: Option<String>,
    pub batch_total: Option<i32>,
    pub status: String,
    pub decided_by_user_id: Option<Uuid>,
    pub decided_by_name: Option<String>,
    pub decided_via: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub deadline: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// (P4-011B) the gateway refused the request at once and kept this approval for a later repeat.
    pub parked: bool,
    /// `ready` | `consumed` | `expired` | `revoked`; only for approved parked approvals.
    pub resume_state: Option<String>,
    pub resume_expires_at: Option<DateTime<Utc>>,
    pub consumed_at: Option<DateTime<Utc>>,
}

const SELECT: &str = "SELECT a.id, a.org_id, a.workspace_id, a.agent_id, ag.name AS agent_name, \
    ag.handle AS agent_handle, a.conversation_id, a.turn_id, a.method, a.host, a.path, a.app_id, \
    a.tool_id, a.connection_id, a.rule_logical_id, a.rule_name, a.summary, a.batch_id, \
    a.batch_label, a.batch_total, a.context_label, a.status, a.decided_by_user_id, u.display_name AS decided_by_name, \
    a.decided_via, a.decided_at, a.deadline, a.created_at, a.parked, a.resume_state, \
    a.resume_expires_at, a.consumed_at \
    FROM approvals a LEFT JOIN agents ag ON ag.id = a.agent_id \
    LEFT JOIN users u ON u.id = a.decided_by_user_id";

/// `$1` = user: the access law for `can_approve` on the approval's workspace (phase 2, RFE-012:
/// workspace owner/approver or org owner/admin; a plain member is excluded).
const MAY_DECIDE_BASE: &str = "EXISTS (SELECT 1 FROM effective_workspace_access ea \
    WHERE ea.workspace_id = a.workspace_id AND ea.org_id = a.org_id AND ea.user_id = $1 \
      AND ea.role IN ('owner', 'approver'))";

/// `$1` = user: a plain workspace member of the approval's workspace (`can_use` but not
/// `can_approve`).
const MEMBER_ONLY: &str = "EXISTS (SELECT 1 FROM effective_workspace_access ea \
    WHERE ea.workspace_id = a.workspace_id AND ea.org_id = a.org_id AND ea.user_id = $1 \
      AND ea.role = 'member')";

/// Does a batch of `agent` exist whose approvals the user can use-but-not-decide (RFE-012)?
pub async fn batch_undecidable(
    pool: &PgPool,
    user: Uuid,
    org: Option<Uuid>,
    ws: Option<Uuid>,
    agent: Uuid,
    batch_id: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM approvals a WHERE a.batch_id = $5 AND a.agent_id = $4 \
         AND {MEMBER_ONLY} AND ($2::uuid IS NULL OR a.org_id = $2) \
         AND ($3::uuid IS NULL OR a.workspace_id = $3) AND NOT {})",
        crate::authz::hidden_direct_conversation_sql("a.conversation_id", "$1", "a.workspace_id")
    )))
    .bind(user)
    .bind(org)
    .bind(ws)
    .bind(agent)
    .bind(batch_id)
    .fetch_one(pool)
    .await?)
}

/// Pending approvals the user can *see the existence of* (`can_use`, conversation visible) but may
/// **not** decide (phase 2, RFE-012: plain members). Used only to answer a refused
/// `approve <code>` with the right ephemeral.
pub async fn list_pending_undecidable_for(
    pool: &PgPool,
    user: Uuid,
    limit: i64,
) -> Result<Vec<ApprovalRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.status = 'pending' \
         AND {MEMBER_ONLY} \
         AND NOT {} \
         ORDER BY a.created_at ASC, a.id ASC LIMIT $2",
        crate::authz::hidden_direct_conversation_sql("a.conversation_id", "$1", "a.workspace_id")
    )))
    .bind(user)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// FIX-112: pending approvals the user may decide whose chat code (`sha256(id)[..5]`, the five
/// bytes `code_prefix` decodes from) matches, found in SQL: the newest approval is as reachable as
/// the oldest, whatever else is pending.
pub async fn list_pending_by_code(
    pool: &PgPool,
    user: Uuid,
    code_prefix: &[u8],
    limit: i64,
) -> Result<Vec<ApprovalRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.status = 'pending' AND {} \
         AND substring(sha256(uuid_send(a.id)) from 1 for 5) = $2 \
         ORDER BY a.created_at ASC, a.id ASC LIMIT $3",
        may_decide()
    )))
    .bind(user)
    .bind(code_prefix)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// [`list_pending_undecidable_for`] narrowed to one chat code (FIX-112).
pub async fn list_pending_undecidable_by_code(
    pool: &PgPool,
    user: Uuid,
    code_prefix: &[u8],
    limit: i64,
) -> Result<Vec<ApprovalRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.status = 'pending' \
         AND {MEMBER_ONLY} \
         AND NOT {} \
         AND substring(sha256(uuid_send(a.id)) from 1 for 5) = $2 \
         ORDER BY a.created_at ASC, a.id ASC LIMIT $3",
        crate::authz::hidden_direct_conversation_sql("a.conversation_id", "$1", "a.workspace_id")
    )))
    .bind(user)
    .bind(code_prefix)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// `MAY_DECIDE` plus conversation visibility (phase 1, RFE-009): an approval naming a direct
/// conversation the user cannot see is neither visible nor decidable.
fn may_decide() -> String {
    format!(
        "({MAY_DECIDE_BASE} AND NOT {})",
        crate::authz::hidden_direct_conversation_sql("a.conversation_id", "$1", "a.workspace_id")
    )
}

/// Can `user` see this approval's conversation (approvals without one: always)?
pub async fn conversation_visible(
    pool: &PgPool,
    user: Uuid,
    workspace_id: Uuid,
    conversation_id: Option<Uuid>,
) -> Result<bool, DbError> {
    match conversation_id {
        None => Ok(true),
        Some(c) => Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT NOT {}",
            crate::authz::hidden_direct_conversation_sql("$2::uuid", "$1", "$3::uuid")
        )))
        .bind(user)
        .bind(c)
        .bind(workspace_id)
        .fetch_one(pool)
        .await?),
    }
}

pub async fn get<'e, E: sqlx::PgExecutor<'e>>(
    pool: E,
    id: Uuid,
) -> Result<Option<ApprovalRow>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE a.id = $1")))
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// (P5-UX-16, C-982) Where the question of an approval came from, resolved from the logical id of
/// the rule that asked: the derived ask rule of an agent's access (a grant), or a policy rule.
#[derive(Debug, Clone)]
pub enum RuleOrigin {
    /// The derived `(ask)` rule of a connection grant.
    Access {
        grant_id: Uuid,
        agent_id: Uuid,
        connection_id: Uuid,
        mode: String,
        ask_tools: Vec<String>,
    },
    /// A policy rule of a workspace (`organization = false`) or of the organization.
    Rule { name: String, organization: bool },
}

pub async fn rule_origin(
    exec: &mut sqlx::PgConnection,
    logical_id: Uuid,
) -> Result<Option<RuleOrigin>, DbError> {
    let derived: Option<(String, Uuid, i32)> = sqlx::query_as(
        "SELECT source_kind, source_id, ordinal FROM derived_rules WHERE logical_id = $1",
    )
    .bind(logical_id)
    .fetch_optional(&mut *exec)
    .await?;
    if let Some((kind, source, ordinal)) = derived {
        if kind != "grant" || ordinal != 1 {
            return Ok(None);
        }
        let g: Option<(Uuid, Option<Uuid>, String, Vec<String>)> = sqlx::query_as(
            "SELECT agent_id, connection_id, COALESCE(mode, 'full'), ask_tools \
             FROM grants WHERE id = $1",
        )
        .bind(source)
        .fetch_optional(&mut *exec)
        .await?;
        return Ok(g.and_then(|(agent_id, connection_id, mode, ask_tools)| {
            connection_id.map(|connection_id| RuleOrigin::Access {
                grant_id: source,
                agent_id,
                connection_id,
                mode,
                ask_tools,
            })
        }));
    }
    let r: Option<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT r.name, g.workspace_id FROM policy_rules r \
         JOIN policy_generations g ON g.id = r.generation_id \
         WHERE r.logical_id = $1 \
         ORDER BY (g.status = 'published') DESC, g.number DESC LIMIT 1",
    )
    .bind(logical_id)
    .fetch_optional(&mut *exec)
    .await?;
    Ok(r.map(|(name, ws)| RuleOrigin::Rule {
        name,
        organization: ws.is_none(),
    }))
}

/// Workspace history, newest first, optional status filter.
pub async fn list_workspace(
    pool: &PgPool,
    viewer: Uuid,
    ws: Uuid,
    status: Option<&str>,
    agent_id: Option<Uuid>,
    after: After,
    fetch: i64,
) -> Result<Vec<ApprovalRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.workspace_id = $6 AND ($2::text IS NULL OR a.status = $2) \
         AND ($7::uuid IS NULL OR a.agent_id = $7) \
         AND ($3::timestamptz IS NULL OR (a.created_at, a.id) < ($3, $4)) \
         AND NOT {} \
         ORDER BY a.created_at DESC, a.id DESC LIMIT $5",
        crate::authz::hidden_direct_conversation_sql("a.conversation_id", "$1", "a.workspace_id")
    )))
    .bind(viewer)
    .bind(status)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .bind(ws)
    .bind(agent_id)
    .fetch_all(pool)
    .await?)
}

/// Pending approvals the user may decide, optionally confined to an org/workspace
/// (API key scope). Oldest first.
pub async fn list_pending_for(
    pool: &PgPool,
    user: Uuid,
    org: Option<Uuid>,
    ws: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ApprovalRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.status = 'pending' AND {} \
         AND ($2::uuid IS NULL OR a.org_id = $2) AND ($3::uuid IS NULL OR a.workspace_id = $3) \
         ORDER BY a.created_at ASC, a.id ASC LIMIT $4",
        may_decide()
    )))
    .bind(user)
    .bind(org)
    .bind(ws)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// (P5-REFINE-8, C-1080) Approvals of a workspace the user may decide that a person decided in the last
/// `days` (the feed's `state=done`), newest decision first.
pub async fn list_decided_for(
    pool: &PgPool,
    user: Uuid,
    ws: Uuid,
    days: i32,
    limit: i64,
) -> Result<Vec<ApprovalRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.status <> 'pending' AND a.decided_at IS NOT NULL AND {} \
         AND a.workspace_id = $2 AND a.decided_at >= now() - make_interval(days => $3) \
         ORDER BY a.decided_at DESC, a.id DESC LIMIT $4",
        may_decide()
    )))
    .bind(user)
    .bind(ws)
    .bind(days)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

async fn notify_tx(
    tx: &mut sqlx::PgConnection,
    id: Uuid,
    ws: Uuid,
    event: &str,
) -> Result<(), DbError> {
    bus::notify(
        &mut *tx,
        Channel::Approval,
        &serde_json::json!({"id": id, "workspace_id": ws, "event": event}),
    )
    .await
    .map_err(|e| DbError::Other(sqlx::Error::Protocol(e.to_string())))
}

/// (FIX-109, `07` §Approval deadlines: "Expiry still denies") A `pending` approval past its
/// deadline is `expired` here, in the caller's transaction, instead of waiting for the sweep
/// (which runs every 10 s and adds a 30 s grace): a decision after the deadline must not win.
/// Returns whether this call expired it.
pub async fn expire_if_overdue(tx: &mut sqlx::PgConnection, id: Uuid) -> Result<bool, DbError> {
    let ws: Option<Uuid> = sqlx::query_scalar(
        "UPDATE approvals SET status = 'expired', decided_at = now(), resolved_at = now(), \
         updated_at = now() WHERE id = $1 AND status = 'pending' AND deadline <= now() \
         RETURNING workspace_id",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(ws) = ws else {
        return Ok(false);
    };
    notify_tx(&mut *tx, id, ws, "expired").await?;
    Ok(true)
}

/// [`expire_if_overdue`] for the pending approvals of one batch of one agent that the user
/// could decide. Returns how many it expired.
pub async fn expire_overdue_of_batch(
    tx: &mut sqlx::PgConnection,
    user: Uuid,
    org: Option<Uuid>,
    ws: Option<Uuid>,
    agent: Uuid,
    batch_id: &str,
) -> Result<usize, DbError> {
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE approvals a SET status = 'expired', decided_at = now(), resolved_at = now(), \
         updated_at = now() \
         WHERE a.status = 'pending' AND a.deadline <= now() AND a.batch_id = $5 \
         AND a.agent_id = $4 AND {} AND ($2::uuid IS NULL OR a.org_id = $2) \
         AND ($3::uuid IS NULL OR a.workspace_id = $3) \
         RETURNING a.id, a.workspace_id",
        may_decide()
    )))
    .bind(user)
    .bind(org)
    .bind(ws)
    .bind(agent)
    .bind(batch_id)
    .fetch_all(&mut *tx)
    .await?;
    for (id, w) in &rows {
        notify_tx(&mut *tx, *id, *w, "expired").await?;
    }
    Ok(rows.len())
}

/// First decision wins: `pending` -> `approved`/`denied`. `None` = already resolved (or past its
/// deadline: (FIX-109) the deadline is part of the statement, so a decision after it cannot win
/// even when the expiry sweep is down).
///
/// Runs inside the caller's transaction (RFE-008): the `approval.decide` audit row commits with
/// the decision, or the decision does not happen.
pub async fn decide(
    tx: &mut sqlx::PgConnection,
    id: Uuid,
    approve: bool,
    user: Uuid,
    via: &str,
    resume_window_secs: u64,
) -> Result<Option<ApprovalRow>, DbError> {
    // (P4-011B-2, C-823) An approved **parked** approval becomes a `ready` single-use grant with
    // its window, in the same statement as the decision.
    let won: Option<Uuid> = sqlx::query_scalar(
        "UPDATE approvals SET status = $2, decided_by_user_id = $3, decided_via = $4, \
         decided_at = now(), resolved_at = now(), updated_at = now(), \
         resume_state = CASE WHEN parked AND $2 = 'approved' THEN 'ready' END, \
         resume_expires_at = CASE WHEN parked AND $2 = 'approved' \
                                  THEN now() + make_interval(secs => $5) END \
         WHERE id = $1 AND status = 'pending' AND deadline > now() \
         RETURNING workspace_id",
    )
    .bind(id)
    .bind(if approve { "approved" } else { "denied" })
    .bind(user)
    .bind(via)
    .bind(resume_window_secs as f64)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(ws) = won else {
        return Ok(None);
    };
    notify_tx(&mut *tx, id, ws, "decided").await?;
    get(&mut *tx, id).await
}

/// (P5-UX-16, C-982) Approves the other pending approvals of the same agent, account and tool, one
/// by one with [`decide`] (so each notifies and starts its resume window), in the caller's
/// transaction. Returns the rows it approved.
#[allow(clippy::too_many_arguments)]
pub async fn approve_same_kind(
    tx: &mut sqlx::PgConnection,
    except: Uuid,
    agent_id: Uuid,
    connection_id: Uuid,
    tool_id: &str,
    user: Uuid,
    via: &str,
    resume_window_secs: u64,
) -> Result<Vec<ApprovalRow>, DbError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM approvals WHERE agent_id = $1 AND connection_id = $2 AND tool_id = $3 \
         AND status = 'pending' AND deadline > now() AND id <> $4 ORDER BY created_at, id",
    )
    .bind(agent_id)
    .bind(connection_id)
    .bind(tool_id)
    .bind(except)
    .fetch_all(&mut *tx)
    .await?;
    let mut out = Vec::new();
    for id in ids {
        if let Some(row) = decide(&mut *tx, id, true, user, via, resume_window_secs).await? {
            out.push(row);
        }
    }
    Ok(out)
}

/// Whether any approval of this batch/agent exists that the user may decide
/// (any status), within the optional key scope.
pub async fn batch_visible(
    pool: &PgPool,
    user: Uuid,
    org: Option<Uuid>,
    ws: Option<Uuid>,
    agent: Uuid,
    batch_id: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM approvals a WHERE a.batch_id = $5 AND a.agent_id = $4 \
         AND {} AND ($2::uuid IS NULL OR a.org_id = $2) \
         AND ($3::uuid IS NULL OR a.workspace_id = $3))",
        may_decide()
    )))
    .bind(user)
    .bind(org)
    .bind(ws)
    .bind(agent)
    .bind(batch_id)
    .fetch_one(pool)
    .await?)
}

/// Decide every pending approval of the batch (same agent) the user may decide.
/// Returns `(id, workspace_id)` of the rows this call resolved.
#[allow(clippy::too_many_arguments)]
pub async fn decide_batch(
    tx: &mut sqlx::PgConnection,
    user: Uuid,
    org: Option<Uuid>,
    ws: Option<Uuid>,
    agent: Uuid,
    batch_id: &str,
    approve: bool,
    via: &str,
    resume_window_secs: u64,
) -> Result<Vec<(Uuid, Uuid, Uuid)>, DbError> {
    let rows: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE approvals a SET status = $6, decided_by_user_id = $1, decided_via = $7, \
         decided_at = now(), resolved_at = now(), updated_at = now(), \
         resume_state = CASE WHEN a.parked AND $6 = 'approved' THEN 'ready' END, \
         resume_expires_at = CASE WHEN a.parked AND $6 = 'approved' \
                                  THEN now() + make_interval(secs => $8) END \
         WHERE a.status = 'pending' AND a.deadline > now() AND a.batch_id = $5 AND a.agent_id = $4 \
         AND {} \
         AND ($2::uuid IS NULL OR a.org_id = $2) AND ($3::uuid IS NULL OR a.workspace_id = $3) \
         RETURNING a.id, a.workspace_id, a.org_id",
        may_decide()
    )))
    .bind(user)
    .bind(org)
    .bind(ws)
    .bind(agent)
    .bind(batch_id)
    .bind(if approve { "approved" } else { "denied" })
    .bind(via)
    .bind(resume_window_secs as f64)
    .fetch_all(&mut *tx)
    .await?;
    for (id, w, _) in &rows {
        notify_tx(&mut *tx, *id, *w, "decided").await?;
    }
    Ok(rows)
}

/// (P4-011B-2, C-823/C-826) Ready grants of the given approvals -> `revoked` (the agent is frozen
/// or the conversation is gone; or an access-removal event). Returns `(id, workspace_id,
/// conversation_id)` of the rows changed and notifies the dashboards.
pub async fn revoke_ready(
    tx: &mut sqlx::PgConnection,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, Uuid, Option<Uuid>)>, DbError> {
    let rows: Vec<(Uuid, Uuid, Option<Uuid>)> = sqlx::query_as(
        "UPDATE approvals SET resume_state = 'revoked', resolved_at = now(), updated_at = now() \
         WHERE id = ANY($1) AND resume_state = 'ready' \
         RETURNING id, workspace_id, conversation_id",
    )
    .bind(ids)
    .fetch_all(&mut *tx)
    .await?;
    for (id, ws, _) in &rows {
        notify_tx(&mut *tx, *id, *ws, "abandoned").await?;
    }
    Ok(rows)
}

/// What ends the `ready` grants and `pending` parked rows of a conversation or an agent.
#[derive(Debug, Clone, Copy)]
pub enum Scope {
    Conversation(Uuid),
    Agent(Uuid),
}

/// (P4-011B-2, C-825/C-826) An approval ended by a revocation hook: a `pending` row that became
/// `abandoned` (`parked` tells whether it was parked: only those get a transcript notice) or a
/// `ready` grant that became `revoked`.
#[derive(Debug, Clone)]
pub struct Ended {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub parked: bool,
    /// `true`: a `ready` grant -> `revoked`; `false`: a `pending` row -> `abandoned`.
    pub grant_revoked: bool,
}

pub(crate) type EndedRow = (Uuid, Uuid, Option<Uuid>, bool);

pub(crate) fn ended(rows: Vec<EndedRow>, grant_revoked: bool) -> Vec<Ended> {
    rows.into_iter()
        .map(|(id, workspace_id, conversation_id, parked)| Ended {
            id,
            workspace_id,
            conversation_id,
            parked,
            grant_revoked,
        })
        .collect()
}

/// (P4-011B-2, C-826) Agent or conversation deletion and a `harness_session_epoch` bump:
/// `pending` **parked** rows -> `abandoned`, `ready` grants -> `revoked`, in the caller's
/// transaction. Held rows are untouched (their gateway ends them). Returns the ended rows; the
/// caller writes the notices and counts the metric (`approvals::resume::end_parked`).
pub async fn end_parked_of(
    tx: &mut sqlx::PgConnection,
    scope: Scope,
) -> Result<Vec<Ended>, DbError> {
    let (col, id) = match scope {
        Scope::Conversation(c) => ("conversation_id", c),
        Scope::Agent(a) => ("agent_id", a),
    };
    let abandoned: Vec<EndedRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE approvals SET status = 'abandoned', decided_at = now(), resolved_at = now(), \
         updated_at = now() WHERE {col} = $1 AND status = 'pending' AND parked \
         RETURNING id, workspace_id, conversation_id, parked"
    )))
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let revoked: Vec<EndedRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE approvals SET resume_state = 'revoked', resolved_at = now(), updated_at = now() \
         WHERE {col} = $1 AND resume_state = 'ready' \
         RETURNING id, workspace_id, conversation_id, parked"
    )))
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let mut all = ended(abandoned, false);
    all.extend(ended(revoked, true));
    for e in &all {
        notify_tx(&mut *tx, e.id, e.workspace_id, "abandoned").await?;
    }
    Ok(all)
}

/// (P4-011B-2, C-828) Ready grants past `resume_expires_at` -> `resume_state = 'expired'`.
/// Returns `(id, workspace_id, conversation_id)`.
pub async fn expire_resume_windows(
    tx: &mut sqlx::PgConnection,
) -> Result<Vec<(Uuid, Uuid, Option<Uuid>)>, DbError> {
    let rows: Vec<(Uuid, Uuid, Option<Uuid>)> = sqlx::query_as(
        "UPDATE approvals SET resume_state = 'expired', resolved_at = now(), updated_at = now() \
         WHERE resume_state = 'ready' AND resume_expires_at <= now() \
         RETURNING id, workspace_id, conversation_id",
    )
    .fetch_all(&mut *tx)
    .await?;
    for (id, ws, _) in &rows {
        notify_tx(&mut *tx, *id, *ws, "expired").await?;
    }
    Ok(rows)
}

/// Parked pending approvals (gauge `aurora_approvals_parked_pending`).
pub async fn count_parked_pending(pool: &PgPool) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM approvals WHERE status = 'pending' AND parked")
            .fetch_one(pool)
            .await?,
    )
}

/// Record the follow-up turn of a decision on the given approvals (`resume_turn_id`).
pub async fn set_resume_turn(
    tx: &mut sqlx::PgConnection,
    ids: &[Uuid],
    turn_id: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE approvals SET resume_turn_id = $2 WHERE id = ANY($1)")
        .bind(ids)
        .bind(turn_id)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

/// What a pending gateway approval depends on (FIX-084): an approval authorizes the held request
/// only under the policy in force when it is released, so the writers that remove or narrow the
/// access it would use end it in their own transaction, as freezing does.
#[derive(Debug, Clone, Copy)]
pub enum Dependency<'a> {
    /// Holds that would use this app connection's credential; `agent` narrows them to one agent
    /// (a grant of that agent), `None` is every agent (the connection itself goes away).
    Connection {
        connection_id: Uuid,
        agent_id: Option<Uuid>,
    },
    /// Holds of this agent that would use a generic secret rather than an app connection (the
    /// row names no secret; revoking one secret ends them all, which is the safe side).
    AgentSecrets { agent_id: Uuid },
    /// Holds that would use any connection of this app (a snapshot that restricts the app).
    App { app_id: &'a str },
}

enum RevokeBind<'a> {
    Uuids(Option<Uuid>, Option<Uuid>),
    Text(&'a str),
}

/// `ready` grants matching `pred` -> `revoked` (C-826: same predicate as the pending rows).
async fn revoke_ready_where(
    tx: &mut sqlx::PgConnection,
    pred: &str,
    binds: RevokeBind<'_>,
) -> Result<Vec<Ended>, DbError> {
    let sql = format!(
        "UPDATE approvals SET resume_state = 'revoked', resolved_at = now(), updated_at = now() \
         WHERE resume_state = 'ready' AND {pred} RETURNING id, workspace_id, conversation_id, parked"
    );
    let q = sqlx::query_as::<_, EndedRow>(sqlx::AssertSqlSafe(sql));
    let rows = match binds {
        RevokeBind::Uuids(a, b) => q.bind(a).bind(b).fetch_all(&mut *tx).await?,
        RevokeBind::Text(t) => q.bind(t).fetch_all(&mut *tx).await?,
    };
    let rows = ended(rows, true);
    for e in &rows {
        notify_tx(&mut *tx, e.id, e.workspace_id, "abandoned").await?;
    }
    Ok(rows)
}

/// `pending` approvals depending on `dep` -> `abandoned`, notifying the waiting gateways.
/// Runs inside the caller's transaction. Returns every row ended (the `pending` ones now
/// `abandoned` and the `ready` grants now `revoked`); the caller writes the notices for the
/// parked ones (`approvals::resume::notify_ended`).
pub async fn abandon_pending_depending_on(
    tx: &mut sqlx::PgConnection,
    dep: Dependency<'_>,
) -> Result<Vec<Ended>, DbError> {
    // The predicate of the dependency, with its binds as `($1, $2)` (`$2` only for a connection).
    let (pred, b1, b2): (&str, Option<Uuid>, Option<Uuid>) = match dep {
        Dependency::Connection {
            connection_id,
            agent_id,
        } => (
            "connection_id = $1 AND ($2::uuid IS NULL OR agent_id = $2)",
            Some(connection_id),
            agent_id,
        ),
        Dependency::AgentSecrets { agent_id } => (
            "agent_id = $1 AND connection_id IS NULL AND $2::uuid IS NULL",
            Some(agent_id),
            None,
        ),
        // `app_id` is text, not a uuid: handled apart below.
        Dependency::App { .. } => ("", None, None),
    };
    let (revoked, rows): (Vec<Ended>, Vec<EndedRow>) = if let Dependency::App { app_id } = dep {
        let pred = "connection_id IN (SELECT id FROM app_connections WHERE app_id = $1)";
        // (P4-011B-2, C-826) The `ready` grants that depend on the same thing are revoked in
        // the same transaction (they are not `pending`, so this runs first and apart).
        let revoked = revoke_ready_where(&mut *tx, pred, RevokeBind::Text(app_id)).await?;
        let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE approvals SET status = 'abandoned', decided_at = now(), resolved_at = now(), \
             updated_at = now() WHERE status = 'pending' AND {pred} \
             RETURNING id, workspace_id, conversation_id, parked"
        )))
        .bind(app_id)
        .fetch_all(&mut *tx)
        .await?;
        (revoked, rows)
    } else {
        let revoked = revoke_ready_where(&mut *tx, pred, RevokeBind::Uuids(b1, b2)).await?;
        let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE approvals SET status = 'abandoned', decided_at = now(), resolved_at = now(), \
             updated_at = now() WHERE status = 'pending' AND {pred} \
             RETURNING id, workspace_id, conversation_id, parked"
        )))
        .bind(b1)
        .bind(b2)
        .fetch_all(&mut *tx)
        .await?;
        (revoked, rows)
    };
    let mut all = ended(rows, false);
    for e in &all {
        // The event follows the status (docs/05 `aurora_approval`): `abandoned`, as on the
        // freeze path (FIX-122).
        notify_tx(&mut *tx, e.id, e.workspace_id, "abandoned").await?;
    }
    all.extend(revoked);
    Ok(all)
}

/// `pending` past `deadline + grace` -> `expired`; a **parked** approval (nobody holds it, so no
/// gateway expires it first) expires at its deadline without grace (P4-011B, C-828). Returns
/// `(id, workspace_id)`.
pub async fn expire_overdue(pool: &PgPool, grace_secs: f64) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    let mut tx = pool.begin().await?;
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE approvals SET status = 'expired', decided_at = now(), resolved_at = now(), \
         updated_at = now() \
         WHERE status = 'pending' \
           AND deadline + CASE WHEN parked THEN interval '0' ELSE make_interval(secs => $1) END < now() \
         RETURNING id, workspace_id",
    )
    .bind(grace_secs)
    .fetch_all(&mut *tx)
    .await?;
    for (id, ws) in &rows {
        notify_tx(&mut tx, *id, *ws, "expired").await?;
    }
    tx.commit().await?;
    Ok(rows)
}

/// `pending` rows whose gateway instance has not been seen for `unseen_secs`
/// (or is unknown) -> `abandoned`. Parked approvals have no holder and are skipped (P4-011B).
/// Returns `(id, workspace_id)`.
pub async fn abandon_unseen(pool: &PgPool, unseen_secs: f64) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    let mut tx = pool.begin().await?;
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE approvals a SET status = 'abandoned', resolved_at = now(), updated_at = now() \
         WHERE a.status = 'pending' AND NOT a.parked AND NOT EXISTS (SELECT 1 FROM gateway_instances g \
           WHERE g.id = a.gateway_instance_id AND g.stopped_at IS NULL \
             AND g.last_seen_at >= now() - make_interval(secs => $1)) \
         RETURNING a.id, a.workspace_id",
    )
    .bind(unseen_secs)
    .fetch_all(&mut *tx)
    .await?;
    for (id, ws) in &rows {
        notify_tx(&mut tx, *id, *ws, "abandoned").await?;
    }
    tx.commit().await?;
    Ok(rows)
}

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
}

const SELECT: &str = "SELECT a.id, a.org_id, a.workspace_id, a.agent_id, ag.name AS agent_name, \
    ag.handle AS agent_handle, a.conversation_id, a.turn_id, a.method, a.host, a.path, a.app_id, \
    a.tool_id, a.connection_id, a.rule_logical_id, a.rule_name, a.summary, a.batch_id, \
    a.batch_label, a.batch_total, a.context_label, a.status, a.decided_by_user_id, u.display_name AS decided_by_name, \
    a.decided_via, a.decided_at, a.deadline, a.created_at \
    FROM approvals a LEFT JOIN agents ag ON ag.id = a.agent_id \
    LEFT JOIN users u ON u.id = a.decided_by_user_id";

/// `$1` = user: the access law for `can_approve` on the approval's workspace (phase 2, RFE-012:
/// workspace owner/approver or org owner/admin; a plain member is excluded).
const MAY_DECIDE_BASE: &str = "EXISTS (SELECT 1 FROM workspaces w \
    JOIN org_members m ON m.org_id = w.org_id AND m.user_id = $1 AND m.status = 'active' \
    LEFT JOIN workspace_access wa ON wa.workspace_id = w.id AND wa.user_id = $1 \
    WHERE w.id = a.workspace_id AND w.org_id = a.org_id \
      AND (m.role IN ('owner', 'admin') OR wa.role IN ('owner', 'approver')))";

/// `$1` = user: a plain workspace member of the approval's workspace (`can_use` but not
/// `can_approve`).
const MEMBER_ONLY: &str = "EXISTS (SELECT 1 FROM workspaces w \
    JOIN org_members m ON m.org_id = w.org_id AND m.user_id = $1 AND m.status = 'active' \
    JOIN workspace_access wa ON wa.workspace_id = w.id AND wa.user_id = $1 \
    WHERE w.id = a.workspace_id AND w.org_id = a.org_id \
      AND m.role = 'member' AND wa.role = 'member')";

/// Does a batch of `agent` exist whose approvals the user can use-but-not-decide (RFE-012)?
pub async fn batch_undecidable(
    pool: &PgPool,
    user: Uuid,
    org: Option<Uuid>,
    ws: Option<Uuid>,
    agent: Uuid,
    batch_id: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM approvals a WHERE a.batch_id = $5 AND a.agent_id = $4 \
         AND {MEMBER_ONLY} AND ($2::uuid IS NULL OR a.org_id = $2) \
         AND ($3::uuid IS NULL OR a.workspace_id = $3) AND NOT {})",
        crate::authz::hidden_direct_conversation_sql("a.conversation_id", "$1", "a.workspace_id")
    ))
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
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE a.status = 'pending' \
         AND {MEMBER_ONLY} \
         AND NOT {} \
         ORDER BY a.created_at ASC, a.id ASC LIMIT $2",
        crate::authz::hidden_direct_conversation_sql("a.conversation_id", "$1", "a.workspace_id")
    ))
    .bind(user)
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
        Some(c) => Ok(sqlx::query_scalar(&format!(
            "SELECT NOT {}",
            crate::authz::hidden_direct_conversation_sql("$2::uuid", "$1", "$3::uuid")
        ))
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
    Ok(sqlx::query_as(&format!("{SELECT} WHERE a.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Workspace history, newest first, optional status filter.
pub async fn list_workspace(
    pool: &PgPool,
    viewer: Uuid,
    ws: Uuid,
    status: Option<&str>,
    after: After,
    fetch: i64,
) -> Result<Vec<ApprovalRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE a.workspace_id = $6 AND ($2::text IS NULL OR a.status = $2) \
         AND ($3::timestamptz IS NULL OR (a.created_at, a.id) < ($3, $4)) \
         AND NOT {} \
         ORDER BY a.created_at DESC, a.id DESC LIMIT $5",
        crate::authz::hidden_direct_conversation_sql("a.conversation_id", "$1", "a.workspace_id")
    ))
    .bind(viewer)
    .bind(status)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .bind(ws)
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
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE a.status = 'pending' AND {} \
         AND ($2::uuid IS NULL OR a.org_id = $2) AND ($3::uuid IS NULL OR a.workspace_id = $3) \
         ORDER BY a.created_at ASC, a.id ASC LIMIT $4",
        may_decide()
    ))
    .bind(user)
    .bind(org)
    .bind(ws)
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

/// First decision wins: `pending` -> `approved`/`denied`. `None` = already resolved.
///
/// Runs inside the caller's transaction (RFE-008): the `approval.decide` audit row commits with
/// the decision, or the decision does not happen.
pub async fn decide(
    tx: &mut sqlx::PgConnection,
    id: Uuid,
    approve: bool,
    user: Uuid,
    via: &str,
) -> Result<Option<ApprovalRow>, DbError> {
    let won: Option<Uuid> = sqlx::query_scalar(
        "UPDATE approvals SET status = $2, decided_by_user_id = $3, decided_via = $4, \
         decided_at = now(), resolved_at = now(), updated_at = now() \
         WHERE id = $1 AND status = 'pending' \
         RETURNING workspace_id",
    )
    .bind(id)
    .bind(if approve { "approved" } else { "denied" })
    .bind(user)
    .bind(via)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(ws) = won else {
        return Ok(None);
    };
    notify_tx(&mut *tx, id, ws, "decided").await?;
    get(&mut *tx, id).await
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
    Ok(sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM approvals a WHERE a.batch_id = $5 AND a.agent_id = $4 \
         AND {} AND ($2::uuid IS NULL OR a.org_id = $2) \
         AND ($3::uuid IS NULL OR a.workspace_id = $3))",
        may_decide()
    ))
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
) -> Result<Vec<(Uuid, Uuid, Uuid)>, DbError> {
    let rows: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(&format!(
        "UPDATE approvals a SET status = $6, decided_by_user_id = $1, decided_via = $7, \
         decided_at = now(), resolved_at = now(), updated_at = now() \
         WHERE a.status = 'pending' AND a.batch_id = $5 AND a.agent_id = $4 AND {} \
         AND ($2::uuid IS NULL OR a.org_id = $2) AND ($3::uuid IS NULL OR a.workspace_id = $3) \
         RETURNING a.id, a.workspace_id, a.org_id",
        may_decide()
    ))
    .bind(user)
    .bind(org)
    .bind(ws)
    .bind(agent)
    .bind(batch_id)
    .bind(if approve { "approved" } else { "denied" })
    .bind(via)
    .fetch_all(&mut *tx)
    .await?;
    for (id, w, _) in &rows {
        notify_tx(&mut *tx, *id, *w, "decided").await?;
    }
    Ok(rows)
}

/// `pending` past `deadline + grace` -> `expired`. Returns `(id, workspace_id)`.
pub async fn expire_overdue(pool: &PgPool, grace_secs: f64) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    let mut tx = pool.begin().await?;
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE approvals SET status = 'expired', decided_at = now(), resolved_at = now(), \
         updated_at = now() \
         WHERE status = 'pending' AND deadline + make_interval(secs => $1) < now() \
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
/// (or is unknown) -> `abandoned`. Returns `(id, workspace_id)`.
pub async fn abandon_unseen(pool: &PgPool, unseen_secs: f64) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    let mut tx = pool.begin().await?;
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE approvals a SET status = 'abandoned', resolved_at = now(), updated_at = now() \
         WHERE a.status = 'pending' AND NOT EXISTS (SELECT 1 FROM gateway_instances g \
           WHERE g.id = a.gateway_instance_id AND g.stopped_at IS NULL \
             AND g.last_seen_at >= now() - make_interval(secs => $1)) \
         RETURNING a.id, a.workspace_id",
    )
    .bind(unseen_secs)
    .fetch_all(&mut *tx)
    .await?;
    for (id, ws) in &rows {
        notify_tx(&mut tx, *id, *ws, "expired").await?;
    }
    tx.commit().await?;
    Ok(rows)
}

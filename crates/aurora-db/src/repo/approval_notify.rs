//! Approval notification plumbing (phase 2, RFE-011a, docs/07 §Approvals "E-mail when no surface
//! is live"): the watchers that make a dashboard/API connection a *live delivery surface*, the
//! pending approvals that have none, their deciders, and the idempotent per-user claims
//! (`approval_notifications`). Reads here never return request bodies or summary fields: the
//! only free text is the approval's sanitized title.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// `U` = SQL expression of the user, `a`/`w` = approval and its workspace aliases: the access law
/// for `can_approve` (workspace owner/approver or org owner/admin) intersected with conversation
/// visibility (RFE-009). Mirrors `approvals::may_decide`.
fn may_decide(user: &str) -> String {
    format!(
        "(EXISTS (SELECT 1 FROM effective_workspace_access ea \
           WHERE ea.workspace_id = w.id AND ea.user_id = {user} \
             AND ea.role IN ('owner', 'approver')) \
         AND NOT {})",
        crate::authz::hidden_direct_conversation_sql("a.conversation_id", user, "a.workspace_id")
    )
}

/// Workspaces whose approvals `user` may decide, optionally confined to an org / workspace (an API
/// key's scope). What an open stream or long-poll of the user watches.
pub async fn watched_workspaces(
    pool: &PgPool,
    user: Uuid,
    org: Option<Uuid>,
    ws: Option<Uuid>,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT w.id FROM workspaces w \
         JOIN effective_workspace_access ea ON ea.workspace_id = w.id AND ea.user_id = $1 \
         WHERE ($2::uuid IS NULL OR w.org_id = $2) AND ($3::uuid IS NULL OR w.id = $3) \
           AND ea.role IN ('owner', 'approver')",
    )
    .bind(user)
    .bind(org)
    .bind(ws)
    .fetch_all(pool)
    .await?)
}

/// `(workspace name, workspace slug, org slug)` for building dashboard links.
pub async fn workspace_labels(
    pool: &PgPool,
    workspace: Uuid,
) -> Result<Option<(String, String, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT w.name, w.slug, o.slug FROM workspaces w JOIN organizations o ON o.id = w.org_id \
         WHERE w.id = $1",
    )
    .bind(workspace)
    .fetch_optional(pool)
    .await?)
}

/// Upsert this replica's watcher rows (`last_seen_at = now()`).
pub async fn heartbeat_watchers(
    pool: &PgPool,
    replica: Uuid,
    pairs: &[(Uuid, Uuid)],
) -> Result<(), DbError> {
    if pairs.is_empty() {
        return Ok(());
    }
    let users: Vec<Uuid> = pairs.iter().map(|p| p.0).collect();
    let wss: Vec<Uuid> = pairs.iter().map(|p| p.1).collect();
    sqlx::query(
        "INSERT INTO approval_watchers (replica_id, user_id, workspace_id, last_seen_at) \
         SELECT $1, u, w, now() FROM UNNEST($2::uuid[], $3::uuid[]) AS t(u, w) \
         ON CONFLICT (replica_id, user_id, workspace_id) DO UPDATE SET last_seen_at = now()",
    )
    .bind(replica)
    .bind(&users)
    .bind(&wss)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop watcher rows not seen for `stale_secs`; returns the number removed.
pub async fn prune_watchers(pool: &PgPool, stale_secs: f64) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM approval_watchers WHERE last_seen_at < now() - make_interval(secs => $1)",
    )
    .bind(stale_secs)
    .execute(pool)
    .await?
    .rows_affected())
}

/// A pending approval (gateway hold or action approval) that nobody is looking at.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Unseen {
    pub id: Uuid,
    /// `gateway` | `action`
    pub kind: String,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub batch_id: Option<String>,
    /// The approval's title (gateway: the summary title; action: the one-line platform summary).
    pub title: String,
    pub rule_name: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub agent_name: String,
    pub workspace_name: String,
    pub workspace_slug: String,
    pub org_name: String,
    pub org_slug: String,
}

/// Pending approvals older than `delay_secs` that are still decidable, have not been served yet
/// and have **no live delivery surface**: (a) no channel card whose `message.post` op finished,
/// and (b) no decider holding a watcher row seen within `ttl_secs`.
pub async fn unseen(
    pool: &PgPool,
    delay_secs: f64,
    ttl_secs: f64,
    limit: i64,
) -> Result<Vec<Unseen>, DbError> {
    let watcher = format!(
        "EXISTS (SELECT 1 FROM approval_watchers wt WHERE wt.workspace_id = a.workspace_id \
           AND wt.last_seen_at > now() - make_interval(secs => $2) AND {})",
        may_decide("wt.user_id")
    );
    let card = |kind: &str| {
        format!(
            "EXISTS (SELECT 1 FROM channel_cards cc JOIN channel_ops co ON co.id = cc.post_op_id \
               WHERE cc.subject_kind = '{kind}' AND cc.subject_id = a.id \
                 AND co.status = 'done' AND cc.state IN ('posted', 'settled'))"
        )
    };
    let gateway = format!(
        "SELECT a.id, 'gateway'::text AS kind, a.org_id, a.workspace_id, a.agent_id, \
            a.conversation_id, a.batch_id, \
            COALESCE(NULLIF(a.summary->>'title', ''), a.method || ' ' || a.host) AS title, \
            a.rule_name, a.deadline AS expires_at, a.created_at, \
            COALESCE(ag.name, 'An agent') AS agent_name, w.name AS workspace_name, \
            w.slug AS workspace_slug, o.name AS org_name, o.slug AS org_slug \
         FROM approvals a JOIN workspaces w ON w.id = a.workspace_id \
         JOIN organizations o ON o.id = w.org_id LEFT JOIN agents ag ON ag.id = a.agent_id \
         WHERE a.status = 'pending' AND a.deadline > now() \
           AND a.created_at <= now() - make_interval(secs => $1) \
           AND NOT EXISTS (SELECT 1 FROM approval_notifications n WHERE n.approval_id = a.id) \
           AND NOT {} AND NOT {watcher}",
        card("approval")
    );
    let action = format!(
        "SELECT a.id, 'action'::text AS kind, w.org_id, a.workspace_id, a.agent_id, \
            a.conversation_id, NULL::text AS batch_id, a.summary AS title, NULL::text AS rule_name, \
            a.expires_at, a.created_at, \
            COALESCE(ag.name, 'An agent') AS agent_name, w.name AS workspace_name, \
            w.slug AS workspace_slug, o.name AS org_name, o.slug AS org_slug \
         FROM action_approvals a JOIN workspaces w ON w.id = a.workspace_id \
         JOIN organizations o ON o.id = w.org_id LEFT JOIN agents ag ON ag.id = a.agent_id \
         WHERE a.status = 'pending' AND a.expires_at > now() \
           AND a.created_at <= now() - make_interval(secs => $1) \
           AND NOT EXISTS (SELECT 1 FROM approval_notifications n WHERE n.approval_id = a.id) \
           AND NOT {} AND NOT {watcher}",
        card("action_approval")
    );
    Ok(sqlx::query_as(&format!(
        "SELECT id, kind, org_id, workspace_id, agent_id, conversation_id, batch_id, title, \
                rule_name, expires_at, \
                agent_name, workspace_name, workspace_slug, org_name, org_slug \
         FROM ({gateway} UNION ALL {action}) u ORDER BY created_at LIMIT $3"
    ))
    .bind(delay_secs)
    .bind(ttl_secs)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Someone who may be e-mailed about an approval.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Decider {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
}

/// The e-mail recipients of an approval: users with an explicit workspace `owner`/`approver`
/// binding (active org members); when the workspace has none, the org owners/admins. Disabled
/// users, users who turned approval e-mails off, and users who cannot see a `direct` conversation
/// the approval names are excluded.
pub async fn deciders(
    pool: &PgPool,
    org: Uuid,
    workspace: Uuid,
    conversation: Option<Uuid>,
) -> Result<Vec<Decider>, DbError> {
    Ok(sqlx::query_as(&format!(
        "WITH explicit AS ( \
             SELECT m.user_id FROM org_members m \
             JOIN workspace_access wa ON wa.user_id = m.user_id AND wa.workspace_id = $2 \
             WHERE m.org_id = $1 AND m.status = 'active' AND wa.role IN ('owner', 'approver') \
             UNION \
             SELECT gm.user_id FROM group_workspace_access b \
             JOIN group_members gm ON gm.group_id = b.group_id AND gm.org_id = b.org_id \
             JOIN org_members m ON m.org_id = gm.org_id AND m.user_id = gm.user_id \
                  AND m.status = 'active' \
             WHERE b.org_id = $1 AND b.workspace_id = $2 AND b.role IN ('owner', 'approver')), \
         recipients AS ( \
             SELECT user_id FROM explicit \
             UNION \
             SELECT m.user_id FROM effective_org_members m \
             WHERE m.org_id = $1 AND m.role IN ('owner', 'admin') \
               AND NOT EXISTS (SELECT 1 FROM explicit)) \
         SELECT u.id AS user_id, u.email::text AS email, u.display_name \
         FROM recipients r JOIN users u ON u.id = r.user_id \
         WHERE u.disabled_at IS NULL AND u.approval_email_enabled \
           AND NOT {} \
         ORDER BY u.id",
        crate::authz::hidden_direct_conversation_sql("$3::uuid", "u.id", "$2")
    ))
    .bind(org)
    .bind(workspace)
    .bind(conversation)
    .fetch_all(pool)
    .await?)
}

/// Claim `(approval, user)`; `true` only for the replica whose insert won (idempotent across
/// replicas). `delivery`: `mail` | `batch` | `digest_mail` | `digest`.
pub async fn claim(
    pool: &PgPool,
    approval: Uuid,
    user: Uuid,
    kind: &str,
    delivery: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO approval_notifications (approval_id, user_id, kind, sent_at, delivery) \
         VALUES ($1, $2, $3, now(), $4) ON CONFLICT (approval_id, user_id) DO NOTHING",
    )
    .bind(approval)
    .bind(user)
    .bind(kind)
    .bind(delivery)
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Has `user` already been served (mailed or folded) for another approval of this batch?
pub async fn batch_served(
    pool: &PgPool,
    user: Uuid,
    agent: Uuid,
    batch_id: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM approval_notifications n JOIN approvals a ON a.id = n.approval_id \
         WHERE n.user_id = $1 AND a.agent_id = $2 AND a.batch_id = $3)",
    )
    .bind(user)
    .bind(agent)
    .bind(batch_id)
    .fetch_one(pool)
    .await?)
}

/// Other pending approvals of the batch (same agent, not yet served to the user): folded into one
/// e-mail. Returns their ids.
pub async fn batch_siblings(
    pool: &PgPool,
    agent: Uuid,
    batch_id: &str,
    except: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT a.id FROM approvals a WHERE a.agent_id = $1 AND a.batch_id = $2 AND a.id <> $3 \
           AND a.status = 'pending' AND a.deadline > now() ORDER BY a.created_at, a.id",
    )
    .bind(agent)
    .bind(batch_id)
    .bind(except)
    .fetch_all(pool)
    .await?)
}

/// Per-user counters for the hourly cap: individual mails sent in the last hour, and whether a
/// digest mail already went out in it.
pub async fn hourly(pool: &PgPool, user: Uuid) -> Result<(i64, bool), DbError> {
    let (mails, digest): (i64, bool) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE delivery = 'mail'), \
                COALESCE(bool_or(delivery = 'digest_mail'), false) \
         FROM approval_notifications WHERE user_id = $1 AND sent_at > now() - interval '1 hour'",
    )
    .bind(user)
    .fetch_one(pool)
    .await?;
    Ok((mails, digest))
}

/// The active, enabled owners and admins of an org with the org's name and slug: the recipients
/// of the attachment quota e-mail notice (FU-P2-015b, `02` `ATTACH_QUOTA_NOTICE`).
pub async fn org_owners(
    pool: &PgPool,
    org: Uuid,
) -> Result<(String, String, Vec<(Uuid, String)>), DbError> {
    let (name, slug): (String, String) =
        sqlx::query_as("SELECT name, slug FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_one(pool)
            .await?;
    let owners = sqlx::query_as(
        "SELECT u.id, u.email::text FROM effective_org_members m JOIN users u ON u.id = m.user_id \
         WHERE m.org_id = $1 AND m.role IN ('owner', 'admin') \
           AND u.disabled_at IS NULL ORDER BY u.id",
    )
    .bind(org)
    .fetch_all(pool)
    .await?;
    Ok((name, slug, owners))
}

/// Delete claims older than `days` (the daily retention pruning).
pub async fn prune_notifications(pool: &PgPool, days: i32) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM approval_notifications WHERE sent_at < now() - make_interval(days => $1)",
    )
    .bind(days)
    .execute(pool)
    .await?
    .rows_affected())
}

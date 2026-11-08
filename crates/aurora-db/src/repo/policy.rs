//! Policy generations (only what tenancy needs).

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

/// Insert an empty generation (`default_action = none`). Published rows get
/// `published_at = now()`; `workspace = None` is the org scope.
pub async fn insert_empty_generation<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    number: i32,
    published: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO policy_generations (id, org_id, workspace_id, number, status, published_at)
         VALUES ($1,$2,$3,$4,$5, CASE WHEN $6 THEN now() END)",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(workspace)
    .bind(number)
    .bind(if published { "published" } else { "draft" })
    .bind(published)
    .execute(exec)
    .await?;
    Ok(())
}

// ---- authoring (BE-033) ----

use super::{After, gateway, split};
use chrono::{DateTime, Utc};
use sqlx::PgConnection;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GenerationRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub number: i32,
    pub status: String,
    pub default_action: String,
    pub uncredentialed_default: String,
    pub uncredentialed_allowlist: serde_json::Value,
    pub open_lane_posture: String,
    pub draft_revision: i64,
    pub published_by: Option<Uuid>,
    pub published_by_name: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GenerationSummaryRow {
    #[sqlx(flatten)]
    pub generation: GenerationRow,
    pub rule_count: i64,
}

const GEN_COLS: &str = "g.id, g.org_id, g.workspace_id, g.number, g.status, g.default_action, \
    g.uncredentialed_default, g.uncredentialed_allowlist, g.open_lane_posture, g.draft_revision, g.published_by, \
    u.display_name AS published_by_name, g.published_at, g.created_at";
const GEN_FROM: &str = "FROM policy_generations g LEFT JOIN users u ON u.id = g.published_by";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RuleRow {
    pub logical_id: Uuid,
    pub priority: i32,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub identities: serde_json::Value,
    pub targets: serde_json::Value,
    pub action: String,
    pub require_approval: bool,
    /// (RFE-011a) hold deadline preference in seconds; only with `require_approval`.
    pub approval_timeout_secs: Option<i32>,
    pub rate_limit: Option<serde_json::Value>,
    pub conditions: serde_json::Value,
}

/// A rule to store; `priority` is assigned from the position.
#[derive(Debug, Clone)]
pub struct NewRule {
    pub logical_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub identities: serde_json::Value,
    pub targets: serde_json::Value,
    pub action: String,
    pub require_approval: bool,
    pub approval_timeout_secs: Option<i32>,
    pub rate_limit: Option<serde_json::Value>,
    pub conditions: serde_json::Value,
}

/// Create the default published (1) + draft (2) generations of a scope if it has none
/// (normally done at org/workspace creation). Idempotent.
pub async fn ensure_scope(
    conn: &mut PgConnection,
    org: Uuid,
    workspace: Option<Uuid>,
) -> Result<(), DbError> {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM policy_generations WHERE org_id = $1 AND workspace_id IS NOT DISTINCT FROM $2",
    )
    .bind(org)
    .bind(workspace)
    .fetch_one(&mut *conn)
    .await?;
    if n == 0 {
        for (number, status) in [(1, "published"), (2, "draft")] {
            sqlx::query(
                "INSERT INTO policy_generations (id, org_id, workspace_id, number, status, published_at) \
                 VALUES ($1,$2,$3,$4,$5, CASE WHEN $5 = 'published' THEN now() END) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(org)
            .bind(workspace)
            .bind(number)
            .bind(status)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

pub async fn generation_by_status<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    status: &str,
) -> Result<Option<GenerationRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {GEN_COLS} {GEN_FROM} \
         WHERE g.org_id = $1 AND g.workspace_id IS NOT DISTINCT FROM $2 AND g.status = $3"
    ))
    .bind(org)
    .bind(workspace)
    .bind(status)
    .fetch_optional(exec)
    .await?)
}

pub async fn generation_by_number<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    number: i32,
) -> Result<Option<GenerationRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {GEN_COLS} {GEN_FROM} \
         WHERE g.org_id = $1 AND g.workspace_id IS NOT DISTINCT FROM $2 AND g.number = $3"
    ))
    .bind(org)
    .bind(workspace)
    .bind(number)
    .fetch_optional(exec)
    .await?)
}

/// The scope's draft, row-locked for the rest of the transaction.
pub async fn lock_draft(
    conn: &mut PgConnection,
    org: Uuid,
    workspace: Option<Uuid>,
) -> Result<Option<GenerationRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {GEN_COLS} {GEN_FROM} \
         WHERE g.org_id = $1 AND g.workspace_id IS NOT DISTINCT FROM $2 AND g.status = 'draft' \
         FOR UPDATE OF g"
    ))
    .bind(org)
    .bind(workspace)
    .fetch_optional(conn)
    .await?)
}

pub async fn rules_of<'e, E: PgExecutor<'e>>(
    exec: E,
    generation_id: Uuid,
) -> Result<Vec<RuleRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT logical_id, priority, name, description, enabled, identities, targets, action, \
                require_approval, approval_timeout_secs, rate_limit, conditions \
         FROM policy_rules WHERE generation_id = $1 ORDER BY priority",
    )
    .bind(generation_id)
    .fetch_all(exec)
    .await?)
}

/// The workspace's published open-lane posture (`enabled` when it has no published generation).
pub async fn published_open_lane_posture<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Uuid,
) -> Result<String, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT open_lane_posture FROM policy_generations \
         WHERE workspace_id = $1 AND status = 'published'",
    )
    .bind(workspace)
    .fetch_optional(exec)
    .await?
    .unwrap_or_else(|| "enabled".to_owned()))
}

/// Replace the draft's settings and rules wholesale; returns the new `draft_revision`.
pub async fn replace_draft(
    conn: &mut PgConnection,
    generation_id: Uuid,
    default_action: &str,
    uncredentialed_default: &str,
    allowlist: &serde_json::Value,
    open_lane_posture: &str,
    rules: &[NewRule],
) -> Result<i64, DbError> {
    let rev: i64 = sqlx::query_scalar(
        "UPDATE policy_generations SET default_action = $2, uncredentialed_default = $3, \
         uncredentialed_allowlist = $4, open_lane_posture = $5, \
         draft_revision = draft_revision + 1, updated_at = now() \
         WHERE id = $1 RETURNING draft_revision",
    )
    .bind(generation_id)
    .bind(default_action)
    .bind(uncredentialed_default)
    .bind(allowlist)
    .bind(open_lane_posture)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM policy_rules WHERE generation_id = $1")
        .bind(generation_id)
        .execute(&mut *conn)
        .await?;
    for (i, r) in rules.iter().enumerate() {
        sqlx::query(
            "INSERT INTO policy_rules (id, generation_id, logical_id, priority, name, description, \
             enabled, identities, targets, action, require_approval, rate_limit, conditions, \
             approval_timeout_secs) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
        )
        .bind(Uuid::now_v7())
        .bind(generation_id)
        .bind(r.logical_id)
        .bind(i as i32)
        .bind(&r.name)
        .bind(&r.description)
        .bind(r.enabled)
        .bind(&r.identities)
        .bind(&r.targets)
        .bind(&r.action)
        .bind(r.require_approval)
        .bind(&r.rate_limit)
        .bind(&r.conditions)
        .bind(r.approval_timeout_secs)
        .execute(&mut *conn)
        .await?;
    }
    Ok(rev)
}

/// Copy settings and rules (same `logical_id`s) from one generation to another.
pub async fn copy_content(conn: &mut PgConnection, from: Uuid, to: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE policy_generations t SET default_action = s.default_action, \
         uncredentialed_default = s.uncredentialed_default, \
         uncredentialed_allowlist = s.uncredentialed_allowlist, \
         open_lane_posture = s.open_lane_posture, updated_at = now() \
         FROM policy_generations s WHERE s.id = $1 AND t.id = $2",
    )
    .bind(from)
    .bind(to)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM policy_rules WHERE generation_id = $1")
        .bind(to)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO policy_rules (id, generation_id, logical_id, priority, name, description, \
         enabled, identities, targets, action, require_approval, rate_limit, conditions, \
         approval_timeout_secs) \
         SELECT gen_random_uuid(), $2, logical_id, priority, name, description, enabled, \
                identities, targets, action, require_approval, rate_limit, conditions, \
                approval_timeout_secs \
         FROM policy_rules WHERE generation_id = $1",
    )
    .bind(from)
    .bind(to)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Promote the (locked) draft to `published`, superseding the previous published
/// generation, and open a fresh draft (number + 1) holding a copy of it.
pub async fn promote_draft(
    conn: &mut PgConnection,
    draft: &GenerationRow,
    by: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE policy_generations SET status = 'superseded', updated_at = now() \
         WHERE org_id = $1 AND workspace_id IS NOT DISTINCT FROM $2 AND status = 'published'",
    )
    .bind(draft.org_id)
    .bind(draft.workspace_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE policy_generations SET status = 'published', published_by = $2, \
         published_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(draft.id)
    .bind(by)
    .execute(&mut *conn)
    .await?;
    let fresh = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO policy_generations (id, org_id, workspace_id, number, status) \
         VALUES ($1,$2,$3,$4,'draft')",
    )
    .bind(fresh)
    .bind(draft.org_id)
    .bind(draft.workspace_id)
    .bind(draft.number + 1)
    .execute(&mut *conn)
    .await?;
    copy_content(conn, draft.id, fresh).await
}

pub async fn list_generations<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    after: After,
    limit: i64,
) -> Result<Vec<GenerationSummaryRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {GEN_COLS}, (SELECT count(*) FROM policy_rules r WHERE r.generation_id = g.id) AS rule_count \
         {GEN_FROM} WHERE g.org_id = $1 AND g.workspace_id IS NOT DISTINCT FROM $2 \
           AND ($3::timestamptz IS NULL OR (g.created_at, g.id) < ($3, $4)) \
         ORDER BY g.created_at DESC, g.id DESC LIMIT $5"
    ))
    .bind(org)
    .bind(workspace)
    .bind(t)
    .bind(id)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// Rows the evaluator needs for one scope: the published generation (or the draft when
/// `use_draft`) plus the derived rules. Same shape the gateway loads.
pub async fn load_scope_rows(
    pool: &sqlx::PgPool,
    org: Uuid,
    workspace: Option<Uuid>,
    use_draft: bool,
) -> Result<gateway::ScopePolicyRows, DbError> {
    let mut rows = gateway::load_scope_policy(pool, org, workspace).await?;
    if use_draft && let Some(d) = generation_by_status(pool, org, workspace, "draft").await? {
        rows.default_action = d.default_action;
        rows.uncredentialed_default = d.uncredentialed_default;
        rows.uncredentialed_allowlist = d.uncredentialed_allowlist;
        rows.open_lane_posture = d.open_lane_posture;
        rows.rules = rules_of(pool, d.id)
            .await?
            .into_iter()
            .map(|r| gateway::PolicyRuleRow {
                logical_id: r.logical_id,
                name: r.name,
                enabled: r.enabled,
                identities: r.identities,
                targets: r.targets,
                action: r.action,
                require_approval: r.require_approval,
                approval_timeout_secs: r.approval_timeout_secs,
                rate_limit: r.rate_limit,
                conditions: r.conditions,
            })
            .collect();
    }
    Ok(rows)
}

// ---- scope lookups used by validation / warnings ----

/// Agents among `ids` that belong to the scope (workspace, or any workspace of the org).
pub async fn agents_in_scope<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT a.id FROM agents a JOIN workspaces w ON w.id = a.workspace_id \
         WHERE a.id = ANY($3) AND w.org_id = $1 AND ($2::uuid IS NULL OR a.workspace_id = $2)",
    )
    .bind(org)
    .bind(workspace)
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// Active org members among `ids` (workspace scope: only those with `can_use` there).
pub async fn users_in_scope<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT m.user_id FROM org_members m \
         WHERE m.org_id = $1 AND m.status = 'active' AND m.user_id = ANY($3) \
           AND ($2::uuid IS NULL OR m.role IN ('owner','admin') \
                OR EXISTS (SELECT 1 FROM workspace_access x \
                           WHERE x.workspace_id = $2 AND x.user_id = m.user_id))",
    )
    .bind(org)
    .bind(workspace)
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// `(id, workspace_id)` of the secrets among `ids` that belong to the org.
pub async fn secret_scopes<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, Option<Uuid>)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, workspace_id FROM secrets WHERE org_id = $1 AND id = ANY($2)")
            .bind(org)
            .bind(ids)
            .fetch_all(exec)
            .await?,
    )
}

/// `(id, workspace_id)` of the app connections among `ids` that belong to the org.
pub async fn connection_scopes<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, Option<Uuid>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, workspace_id FROM app_connections WHERE org_id = $1 AND id = ANY($2)",
    )
    .bind(org)
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// The workspace's organization (None if the workspace does not exist).
pub async fn workspace_org<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM workspaces WHERE id = $1")
            .bind(workspace)
            .fetch_optional(exec)
            .await?,
    )
}

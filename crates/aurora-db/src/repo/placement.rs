//! New-user placement (P4-ONB-1, D-140, SEC-79): the saved instance placement and personal
//! workspaces.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlacementRow {
    pub mode: String,
    pub default_org_id: Option<Uuid>,
    pub default_org_role: String,
    pub shared_workspaces: serde_json::Value,
    pub personal_enabled: bool,
    pub personal_role: String,
    pub name_template: String,
    pub starter_grants: serde_json::Value,
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "mode, default_org_id, default_org_role, shared_workspaces, personal_enabled, \
    personal_role, name_template, starter_grants, updated_by, updated_at";

pub async fn get<'e, E: PgExecutor<'e>>(exec: E) -> Result<Option<PlacementRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM instance_placement"
    )))
    .fetch_optional(exec)
    .await?)
}

/// Insert or replace the single placement row.
pub async fn save<'e, E: PgExecutor<'e>>(
    exec: E,
    p: &PlacementRow,
    by: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "WITH setup AS (UPDATE instance_setup SET placement_pending = false, updated_at = now()
                         WHERE placement_pending)
         INSERT INTO instance_placement (singleton, mode, default_org_id, default_org_role,
            shared_workspaces, personal_enabled, personal_role, name_template, starter_grants,
            updated_by, updated_at)
         VALUES (true, $1,$2,$3,$4,$5,$6,$7,$8,$9, now())
         ON CONFLICT (singleton) DO UPDATE SET mode = $1, default_org_id = $2,
            default_org_role = $3, shared_workspaces = $4, personal_enabled = $5,
            personal_role = $6, name_template = $7, starter_grants = $8, updated_by = $9,
            updated_at = now()",
    )
    .bind(&p.mode)
    .bind(p.default_org_id)
    .bind(&p.default_org_role)
    .bind(&p.shared_workspaces)
    .bind(p.personal_enabled)
    .bind(&p.personal_role)
    .bind(&p.name_template)
    .bind(&p.starter_grants)
    .bind(by)
    .execute(exec)
    .await?;
    Ok(())
}

/// (P5-ADMDEL, D-234) Remove deleted workspaces from `shared_workspaces`: the owner's and the
/// instance admin's delete paths call it in the delete's own transaction, before the rows go.
/// `workspace_ids` are workspace ids; `org_id` additionally drops every workspace of that
/// organization (an organization delete). No-op without a placement row or a match.
pub async fn prune_shared_workspaces<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace_ids: &[Uuid],
    org_id: Option<Uuid>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE instance_placement SET shared_workspaces = COALESCE((
             SELECT jsonb_agg(e) FROM jsonb_array_elements(shared_workspaces) e
             WHERE NOT ((e->>'workspace_id')::uuid = ANY($1)
                        OR EXISTS (SELECT 1 FROM workspaces w
                                   WHERE w.id = (e->>'workspace_id')::uuid AND w.org_id = $2))
         ), '[]'::jsonb)
         WHERE jsonb_typeof(shared_workspaces) = 'array'
           AND EXISTS (SELECT 1 FROM jsonb_array_elements(shared_workspaces) e
                       WHERE (e->>'workspace_id')::uuid = ANY($1)
                          OR EXISTS (SELECT 1 FROM workspaces w
                                     WHERE w.id = (e->>'workspace_id')::uuid AND w.org_id = $2))",
    )
    .bind(workspace_ids)
    .bind(org_id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Serialize placement work per organization (concurrent first sign-ins).
pub async fn lock_org<'e, E: PgExecutor<'e>>(exec: E, org: Uuid) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('placement:' || $1::text, 0))")
        .bind(org.to_string())
        .execute(exec)
        .await?;
    Ok(())
}

/// The user's personal workspace in `org`, found **only** by the recorded owner id (SEC-79).
pub async fn personal_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM workspaces WHERE org_id = $1 AND personal_owner_user_id = $2",
    )
    .bind(org)
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

/// (FIX-097) Is the workspace a personal one? The sticky `personal` marker, not the owner id:
/// that is nulled when the person's account is deleted, and the workspace must still never be
/// handed out as a shared workspace.
pub async fn is_personal<'e, E: PgExecutor<'e>>(exec: E, workspace: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT personal OR personal_owner_user_id IS NOT NULL FROM workspaces WHERE id = $1",
    )
    .bind(workspace)
    .fetch_optional(exec)
    .await?
    .unwrap_or(false))
}

/// (P5-UX-11, C-947) `(workspace, owner user, owner's name)` for those of `ids` that are someone's
/// personal workspace.
pub async fn personal_owners_of<'e, E: PgExecutor<'e>>(
    exec: E,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, Uuid, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT w.id, u.id, u.display_name FROM workspaces w \
         JOIN users u ON u.id = w.personal_owner_user_id WHERE w.id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// The user a workspace is the personal workspace of, if any.
pub async fn personal_owner_of<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT personal_owner_user_id FROM workspaces WHERE id = $1")
            .bind(workspace)
            .fetch_optional(exec)
            .await?
            .flatten(),
    )
}

pub async fn set_personal_owner<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Uuid,
    user: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE workspaces SET personal_owner_user_id = $2, personal = true WHERE id = $1")
        .bind(workspace)
        .bind(user)
        .execute(exec)
        .await?;
    Ok(())
}

/// `(slug, name)` of an organization.
pub async fn org_brief<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Option<(String, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT slug, name FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_optional(exec)
            .await?,
    )
}

/// What the admin may pick in `org`: ordinary workspaces (never personal ones) and org-level
/// credentials. Capped; names only.
pub async fn choices<'e, E: PgExecutor<'e> + Copy>(exec: E, org: Uuid) -> Result<Choices, DbError> {
    let workspaces = sqlx::query_as(
        "SELECT id, slug, name FROM workspaces WHERE org_id = $1 AND NOT personal
           AND personal_owner_user_id IS NULL
         ORDER BY name, id LIMIT 500",
    )
    .bind(org)
    .fetch_all(exec)
    .await?;
    let secrets = sqlx::query_as(
        "SELECT id, name, kind FROM secrets WHERE org_id = $1 AND workspace_id IS NULL
         AND status = 'active' ORDER BY name, id LIMIT 500",
    )
    .bind(org)
    .fetch_all(exec)
    .await?;
    let connections = sqlx::query_as(
        "SELECT id, label, app_id FROM app_connections WHERE org_id = $1 AND workspace_id IS NULL
         AND status <> 'orphaned' ORDER BY label, id LIMIT 500",
    )
    .bind(org)
    .fetch_all(exec)
    .await?;
    Ok(Choices {
        workspaces,
        secrets,
        connections,
    })
}

#[derive(Debug, Clone)]
pub struct Choices {
    pub workspaces: Vec<(Uuid, String, String)>,
    pub secrets: Vec<(Uuid, String, String)>,
    pub connections: Vec<(Uuid, String, String)>,
}

/// Whether a placement has still to be saved on an instance installed with this release
/// (FIX-116): ordinary users are refused with `instance_setup_pending` until it is. A missing
/// row (never written) counts as not pending.
pub async fn setup_pending<'e, E: PgExecutor<'e>>(exec: E) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT placement_pending FROM instance_setup")
            .fetch_optional(exec)
            .await?
            .unwrap_or(false),
    )
}

/// Default agents of the personal workspaces of `org` (a person who still owns one: the owner
/// id is cleared when the account is deleted), oldest first (C-922).
pub async fn personal_default_agents<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT a.id FROM workspaces w JOIN agents a ON a.workspace_id = w.id AND a.is_default
         WHERE w.org_id = $1 AND w.personal_owner_user_id IS NOT NULL
         ORDER BY w.created_at, w.id",
    )
    .bind(org)
    .fetch_all(exec)
    .await?)
}

/// How many personal workspaces of `org` have a default agent that lacks at least one of the
/// starter credentials (C-922, `personal_workspaces_without_starter`).
pub async fn count_personal_without_starter<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    secret_ids: &[Uuid],
    connection_ids: &[Uuid],
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM workspaces w JOIN agents a ON a.workspace_id = w.id AND a.is_default
         WHERE w.org_id = $1 AND w.personal_owner_user_id IS NOT NULL
           AND (EXISTS (SELECT 1 FROM unnest($2::uuid[]) s(id) WHERE NOT EXISTS
                  (SELECT 1 FROM grants g WHERE g.agent_id = a.id AND g.secret_id = s.id))
             OR EXISTS (SELECT 1 FROM unnest($3::uuid[]) c(id) WHERE NOT EXISTS
                  (SELECT 1 FROM grants g WHERE g.agent_id = a.id AND g.connection_id = c.id)))",
    )
    .bind(org)
    .bind(secret_ids)
    .bind(connection_ids)
    .fetch_one(exec)
    .await?)
}

/// Active LLM keys of the agent's organization (org-level) or workspace that are **not** granted
/// to `agent`, oldest first (C-1062: what "give this agent a key" can offer).
pub async fn ungranted_llm_keys<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    org: Uuid,
    workspace: Uuid,
) -> Result<Vec<crate::repo::hosted::LlmCandidate>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.name, s.status, s.workspace_id, s.llm_provider, s.llm_endpoint_id,
                s.llm_auth_mode, s.origin, s.value_version
         FROM secrets s
         WHERE s.kind = 'llm' AND s.org_id = $2 AND s.status = 'active'
           AND (s.workspace_id IS NULL OR s.workspace_id = $3)
           AND s.llm_provider IS NOT NULL AND s.llm_endpoint_id IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM grants g WHERE g.secret_id = s.id AND g.agent_id = $1)
         ORDER BY s.created_at, s.id LIMIT 200",
    )
    .bind(agent)
    .bind(org)
    .bind(workspace)
    .fetch_all(exec)
    .await?)
}

/// The agents starter grants reach in `org`, oldest first, at most `limit` (C-1061): every hosted
/// agent of every workspace, plus the default agent of a personal workspace whatever its kind
/// (the sign-in has always given it the grants, C-922).
pub async fn hosted_agents_of_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT a.id FROM agents a WHERE a.org_id = $1 AND (a.kind = 'hosted' OR (a.is_default AND
           EXISTS (SELECT 1 FROM workspaces w WHERE w.id = a.workspace_id
                     AND w.personal_owner_user_id IS NOT NULL)))
         ORDER BY a.created_at, a.id LIMIT $2",
    )
    .bind(org)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// How many hosted agents of `org` lack at least one of the starter credentials
/// (C-1061, `agents_without_starter`).
pub async fn count_agents_without_starter<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    secret_ids: &[Uuid],
    connection_ids: &[Uuid],
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM agents a WHERE a.org_id = $1 AND (a.kind = 'hosted' OR (a.is_default AND
           EXISTS (SELECT 1 FROM workspaces w WHERE w.id = a.workspace_id
                     AND w.personal_owner_user_id IS NOT NULL)))
           AND (EXISTS (SELECT 1 FROM unnest($2::uuid[]) s(id) WHERE NOT EXISTS
                  (SELECT 1 FROM grants g WHERE g.agent_id = a.id AND g.secret_id = s.id))
             OR EXISTS (SELECT 1 FROM unnest($3::uuid[]) c(id) WHERE NOT EXISTS
                  (SELECT 1 FROM grants g WHERE g.agent_id = a.id AND g.connection_id = c.id)))",
    )
    .bind(org)
    .bind(secret_ids)
    .bind(connection_ids)
    .fetch_one(exec)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PersonalOrg {
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub owner_email: String,
    pub created_at: DateTime<Utc>,
}

/// Orgs the old `personal_org` sign-up made: named `<name>'s org`, one member, who is its owner
/// and has it as their default organization. (Renamed or shared orgs no longer qualify.)
pub async fn list_personal_orgs<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<PersonalOrg>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT o.id AS org_id, o.slug, o.name, u.email::text AS owner_email, o.created_at
         {PERSONAL_ORGS} ORDER BY o.created_at DESC, o.id DESC LIMIT $1"
    )))
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

pub async fn count_personal_orgs<'e, E: PgExecutor<'e>>(exec: E) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) {PERSONAL_ORGS}"
    )))
    .fetch_one(exec)
    .await?)
}

const PERSONAL_ORGS: &str = "FROM organizations o
    JOIN org_members m ON m.org_id = o.id AND m.role = 'owner'
    JOIN users u ON u.id = m.user_id AND u.default_org_id = o.id
    WHERE o.name LIKE '%''s org'
      AND (SELECT count(*) FROM org_members x WHERE x.org_id = o.id) = 1";

/// (FIX-234 review 4, D-480, C-1161) The person is (again) an active member of `org`: bind them
/// as owner of their personal workspace when the binding is missing, and raise and activate a
/// lower or suspended one — the migration's backfill at restoration time, so a personal
/// workspace is never left ownerless while its person is away (their access stays gated by the
/// active membership exactly as for any binding). Writes the audit row
/// `workspace_access.personal_owner` (actor `system`, `metadata.from`: the old role or `none`)
/// and answers whether anything changed. Call inside the restoring transaction.
pub async fn rebind_personal_owner(
    conn: &mut sqlx::PgConnection,
    org: Uuid,
    user: Uuid,
) -> Result<bool, DbError> {
    let ws: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workspaces WHERE org_id = $1 AND personal_owner_user_id = $2",
    )
    .bind(org)
    .bind(user)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(ws) = ws else {
        return Ok(false);
    };
    let prev: Option<(String, String)> = sqlx::query_as(
        "SELECT role, status FROM workspace_access WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(user)
    .fetch_optional(&mut *conn)
    .await?;
    match prev {
        // Already the active owner: nothing to change.
        Some((r, s)) if r == "owner" && s == "active" => Ok(false),
        // A lower or suspended binding: raise to owner and set active (a person's own
        // workspace never suspends them).
        Some((r, _)) => {
            sqlx::query(
                "UPDATE workspace_access SET role = 'owner', status = 'active', updated_at = now()
                 WHERE workspace_id = $1 AND user_id = $2",
            )
            .bind(ws)
            .bind(user)
            .execute(&mut *conn)
            .await?;
            audit_personal_owner(&mut *conn, org, ws, user, &r).await?;
            Ok(true)
        }
        // No binding at all: insert the owner.
        None => {
            sqlx::query(
                "INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1,$2,'owner')",
            )
            .bind(ws)
            .bind(user)
            .execute(&mut *conn)
            .await?;
            audit_personal_owner(&mut *conn, org, ws, user, "none").await?;
            Ok(true)
        }
    }
}

async fn audit_personal_owner(
    conn: &mut sqlx::PgConnection,
    org: Uuid,
    ws: Uuid,
    user: Uuid,
    from: &str,
) -> Result<(), DbError> {
    let metadata = serde_json::json!({ "from": from });
    super::audit::insert(
        &mut *conn,
        &super::audit::NewAudit {
            org_id: Some(org),
            workspace_id: Some(ws),
            actor_kind: "system",
            actor_id: None,
            actor_label: "system",
            action: "workspace_access.personal_owner",
            target_kind: Some("user"),
            target_id: Some(&user.to_string()),
            outcome: "success",
            source: "system",
            ip: None,
            metadata: &metadata,
        },
    )
    .await?;
    Ok(())
}

//! Directory groups (phase 4, P4-GRP-1, D-130, D-131, SEC-71): rows, membership, workspace
//! bindings, the impact view and the read models the dashboard needs.
//!
//! This module only stores and reads. **It never decides access**: roles are derived by
//! `crate::authz` (`effective_access`, the `effective_*` views). A group never creates,
//! reactivates or removes an org membership: `add_member` inserts a row that the composite
//! foreign key to `org_members` accepts only for an existing member of the group's org.

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

/// Limits (`02 §Limits`).
pub const GROUPS_PER_ORG: i64 = 500;
pub const GROUP_MEMBERS_MAX: i64 = 5_000;
pub const GROUPS_PER_USER: i64 = 100;
pub const GROUP_BINDINGS_PER_GROUP: i64 = 200;
pub const GROUP_NAME_MAX: usize = 80;
pub const GROUP_DESCRIPTION_MAX: usize = 280;
pub const GROUP_IMPACT_LIST_MAX: i64 = 200;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GroupRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub source: String,
    pub org_role: Option<String>,
    pub revision: i64,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub member_count: i64,
    pub workspace_count: i64,
}

const SEL: &str = "SELECT g.id, g.org_id, g.name::text AS name, g.description, g.source, \
    g.org_role, g.revision, g.created_by, g.created_at, g.updated_at, \
    (SELECT count(*) FROM group_members m WHERE m.group_id = g.id) AS member_count, \
    (SELECT count(*) FROM group_workspace_access b WHERE b.group_id = g.id) AS workspace_count \
    FROM groups g";

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<GroupRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SEL} WHERE g.id = $1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

/// Lock the group row (`FOR UPDATE`) so membership/binding limit checks do not race.
pub async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<Option<GroupRow>, DbError> {
    let locked: Option<Uuid> = sqlx::query_scalar("SELECT id FROM groups WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    match locked {
        Some(id) => get(&mut *conn, id).await,
        None => Ok(None),
    }
}

/// Serialize group creation per org so `GROUPS_PER_ORG` holds under concurrency.
pub async fn lock_org(conn: &mut PgConnection, org: Uuid) -> Result<(), DbError> {
    sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR UPDATE")
        .bind(org)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(())
}

pub async fn count_in_org<'e, E: PgExecutor<'e>>(exec: E, org: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM groups WHERE org_id = $1")
            .bind(org)
            .fetch_one(exec)
            .await?,
    )
}

/// Org groups, newest first. `q` is a case-insensitive name substring, `source` filters on
/// `manual`/`scim`, `member` keeps the groups that contain that user.
#[allow(clippy::too_many_arguments)]
pub async fn list<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    q: Option<&str>,
    source: Option<&str>,
    member: Option<Uuid>,
    after: After,
    fetch: i64,
) -> Result<Vec<GroupRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "{SEL} WHERE g.org_id = $1 \
           AND ($2::text IS NULL OR strpos(lower(g.name::text), lower($2)) > 0) \
           AND ($3::text IS NULL OR g.source = $3) \
           AND ($4::uuid IS NULL OR EXISTS (SELECT 1 FROM group_members m \
                  WHERE m.group_id = g.id AND m.user_id = $4)) \
           AND ($5::timestamptz IS NULL OR (g.created_at, g.id) < ($5, $6)) \
         ORDER BY g.created_at DESC, g.id DESC LIMIT $7"
    ))
    .bind(org)
    .bind(q)
    .bind(source)
    .bind(member)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    org: Uuid,
    name: &str,
    description: Option<&str>,
    source: &str,
    org_role: Option<&str>,
    created_by: Option<Uuid>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO groups (id, org_id, name, description, source, org_role, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(org)
    .bind(name)
    .bind(description)
    .bind(source)
    .bind(org_role)
    .bind(created_by)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Optimistic update: applies only when `revision` matches; `None` = stale. `description` and
/// `org_role` are `Some(None)` to clear.
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    revision: i64,
    name: Option<&str>,
    description: Option<Option<&str>>,
    org_role: Option<Option<&str>>,
) -> Result<Option<()>, DbError> {
    let r = sqlx::query(
        "UPDATE groups SET \
           name = COALESCE($3, name), \
           description = CASE WHEN $4 THEN $5 ELSE description END, \
           org_role = CASE WHEN $6 THEN $7 ELSE org_role END, \
           revision = revision + 1, updated_at = now() \
         WHERE id = $1 AND revision = $2",
    )
    .bind(id)
    .bind(revision)
    .bind(name)
    .bind(description.is_some())
    .bind(description.flatten())
    .bind(org_role.is_some())
    .bind(org_role.flatten())
    .execute(&mut *conn)
    .await?;
    Ok((r.rows_affected() > 0).then_some(()))
}

/// Deletes the group; members and bindings go with it (cascade). Policy rules are never
/// rewritten (SEC-71).
pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM groups WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected()
        > 0)
}

// ---- membership ----

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MemberRow {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    /// The user's org membership status (`suspended` members stay listed, derive nothing).
    pub status: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

pub async fn list_members<'e, E: PgExecutor<'e>>(
    exec: E,
    group: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<MemberRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(
        "SELECT gm.user_id, u.email::text AS email, u.display_name, om.status, gm.source, \
                gm.created_at \
           FROM group_members gm \
           JOIN users u ON u.id = gm.user_id \
           JOIN org_members om ON om.org_id = gm.org_id AND om.user_id = gm.user_id \
          WHERE gm.group_id = $1 \
            AND ($2::timestamptz IS NULL OR (gm.created_at, gm.user_id) < ($2, $3)) \
          ORDER BY gm.created_at DESC, gm.user_id DESC LIMIT $4",
    )
    .bind(group)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

pub async fn member_count<'e, E: PgExecutor<'e>>(exec: E, group: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM group_members WHERE group_id = $1")
            .bind(group)
            .fetch_one(exec)
            .await?,
    )
}

pub async fn is_member<'e, E: PgExecutor<'e>>(
    exec: E,
    group: Uuid,
    user: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM group_members WHERE group_id = $1 AND user_id = $2)",
    )
    .bind(group)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

/// Number of groups of `org` that contain `user` (`GROUPS_PER_USER`).
pub async fn groups_of_user_count<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM group_members WHERE org_id = $1 AND user_id = $2",
    )
    .bind(org)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

/// Adds a member. Returns whether a row was inserted (idempotent). A user who is not a member
/// of the group's org is refused by the composite foreign key (`ForeignKeyViolation`): a group
/// never creates a membership.
pub async fn add_member(
    conn: &mut PgConnection,
    group: Uuid,
    org: Uuid,
    user: Uuid,
    source: &str,
    added_by: Option<Uuid>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO group_members (group_id, org_id, user_id, source, added_by) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (group_id, user_id) DO NOTHING",
    )
    .bind(group)
    .bind(org)
    .bind(user)
    .bind(source)
    .bind(added_by)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0)
}

pub async fn remove_member(
    conn: &mut PgConnection,
    group: Uuid,
    user: Uuid,
) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND user_id = $2")
            .bind(group)
            .bind(user)
            .execute(&mut *conn)
            .await?
            .rows_affected()
            > 0,
    )
}

pub async fn member_ids<'e, E: PgExecutor<'e>>(exec: E, group: Uuid) -> Result<Vec<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT user_id FROM group_members WHERE group_id = $1")
            .bind(group)
            .fetch_all(exec)
            .await?,
    )
}

/// The caller's own groups across orgs (`/v1/me`): active memberships only.
pub async fn groups_of_user<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Vec<(Uuid, String, Uuid)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT g.id, g.name::text, g.org_id \
           FROM group_members gm \
           JOIN groups g ON g.id = gm.group_id \
           JOIN org_members om ON om.org_id = gm.org_id AND om.user_id = gm.user_id \
                AND om.status = 'active' \
          WHERE gm.user_id = $1 ORDER BY g.name, g.id",
    )
    .bind(user)
    .fetch_all(exec)
    .await?)
}

// ---- bindings ----

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BindingRow {
    pub group_id: Uuid,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub workspace_slug: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

pub async fn list_bindings<'e, E: PgExecutor<'e>>(
    exec: E,
    group: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<BindingRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(
        "SELECT b.group_id, b.workspace_id, w.name AS workspace_name, w.slug AS workspace_slug, \
                b.role, b.created_at \
           FROM group_workspace_access b JOIN workspaces w ON w.id = b.workspace_id \
          WHERE b.group_id = $1 \
            AND ($2::timestamptz IS NULL OR (b.created_at, b.workspace_id) < ($2, $3)) \
          ORDER BY b.created_at DESC, b.workspace_id DESC LIMIT $4",
    )
    .bind(group)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

pub async fn bound_workspaces<'e, E: PgExecutor<'e>>(
    exec: E,
    group: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT workspace_id FROM group_workspace_access WHERE group_id = $1")
            .bind(group)
            .fetch_all(exec)
            .await?,
    )
}

pub async fn binding_count<'e, E: PgExecutor<'e>>(exec: E, group: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM group_workspace_access WHERE group_id = $1")
            .bind(group)
            .fetch_one(exec)
            .await?,
    )
}

/// The role of the existing binding, if any.
pub async fn binding_role<'e, E: PgExecutor<'e>>(
    exec: E,
    group: Uuid,
    ws: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT role FROM group_workspace_access WHERE group_id = $1 AND workspace_id = $2",
    )
    .bind(group)
    .bind(ws)
    .fetch_optional(exec)
    .await?)
}

/// Binds or changes the role. The composite foreign keys refuse a workspace of another org.
pub async fn bind(
    conn: &mut PgConnection,
    group: Uuid,
    ws: Uuid,
    org: Uuid,
    role: &str,
    granted_by: Option<Uuid>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO group_workspace_access (group_id, workspace_id, org_id, role, granted_by) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (group_id, workspace_id) DO UPDATE \
           SET role = EXCLUDED.role, granted_by = EXCLUDED.granted_by, updated_at = now()",
    )
    .bind(group)
    .bind(ws)
    .bind(org)
    .bind(role)
    .bind(granted_by)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn unbind(conn: &mut PgConnection, group: Uuid, ws: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "DELETE FROM group_workspace_access WHERE group_id = $1 AND workspace_id = $2",
    )
    .bind(group)
    .bind(ws)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WorkspaceGroupBinding {
    pub group_id: Uuid,
    pub name: String,
    pub source: String,
    pub member_count: i64,
    pub role: String,
}

/// The groups bound to a workspace (`GET …/access?expand=groups`).
pub async fn workspace_bindings<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
) -> Result<Vec<WorkspaceGroupBinding>, DbError> {
    Ok(sqlx::query_as(
        "SELECT g.id AS group_id, g.name::text AS name, g.source, b.role, \
                (SELECT count(*) FROM group_members m WHERE m.group_id = g.id) AS member_count \
           FROM group_workspace_access b JOIN groups g ON g.id = b.group_id \
          WHERE b.workspace_id = $1 ORDER BY g.name, g.id",
    )
    .bind(ws)
    .fetch_all(exec)
    .await?)
}

// ---- derived members ----

#[derive(Debug, Clone)]
pub struct DerivedMember {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub sources: Vec<crate::authz::AccessSource>,
}

/// Everyone whose access to the workspace comes (at least partly) from a group or an org role:
/// the effective role and its sources, once per person. Ordered `(created_at desc, user_id desc)`
/// by the member's org membership time.
pub async fn derived_members<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<DerivedMember>, DbError> {
    let (t, id) = split(after);
    type Row = (
        Uuid,
        String,
        String,
        String,
        DateTime<Utc>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        serde_json::Value,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT ew.user_id, u.email::text, u.display_name, ew.role, om.created_at, om.role, \
                wa.role, wa.idp_source, wa.base_role, \
                COALESCE((SELECT jsonb_agg(jsonb_build_object( \
                              'group', g.id, 'org_role', g.org_role, 'ws_role', b.role)) \
                            FROM group_members gm \
                            JOIN groups g ON g.id = gm.group_id \
                            LEFT JOIN group_workspace_access b \
                                   ON b.group_id = g.id AND b.workspace_id = ew.workspace_id \
                           WHERE gm.org_id = ew.org_id AND gm.user_id = ew.user_id \
                             AND (g.org_role IS NOT NULL OR b.role IS NOT NULL)), '[]'::jsonb) \
           FROM effective_workspace_access ew \
           JOIN users u ON u.id = ew.user_id \
           JOIN org_members om ON om.org_id = ew.org_id AND om.user_id = ew.user_id \
           LEFT JOIN workspace_access wa \
                  ON wa.workspace_id = ew.workspace_id AND wa.user_id = ew.user_id \
          WHERE ew.workspace_id = $1 \
            AND (om.role IN ('owner', 'admin') \
                 OR EXISTS (SELECT 1 FROM group_members gm \
                              JOIN groups g ON g.id = gm.group_id \
                              LEFT JOIN group_workspace_access b \
                                     ON b.group_id = g.id AND b.workspace_id = ew.workspace_id \
                             WHERE gm.org_id = ew.org_id AND gm.user_id = ew.user_id \
                               AND (g.org_role = 'admin' OR b.role IS NOT NULL))) \
            AND ($2::timestamptz IS NULL OR (om.created_at, ew.user_id) < ($2, $3)) \
          ORDER BY om.created_at DESC, ew.user_id DESC LIMIT $4",
    )
    .bind(ws)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(user_id, email, display_name, role, created_at, org_role, wa, idp, base, groups)| {
                DerivedMember {
                    user_id,
                    email,
                    display_name,
                    role,
                    created_at,
                    sources: crate::authz::build_sources(
                        &org_role,
                        wa.as_deref(),
                        idp,
                        base.as_deref(),
                        &groups,
                    ),
                }
            },
        )
        .collect())
}

// ---- offboarding: which groups give a person access ----

/// For each workspace in `workspaces`, the groups (id, name) that currently give `user` access:
/// a binding on that workspace, or an `admin` org-role mapping (every workspace of the org).
pub async fn via_groups<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
    workspaces: &[Uuid],
) -> Result<Vec<(Uuid, Uuid, String)>, DbError> {
    if workspaces.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT w.id, g.id, g.name::text \
           FROM unnest($3::uuid[]) AS w(id) \
           JOIN group_members gm ON gm.org_id = $1 AND gm.user_id = $2 \
           JOIN groups g ON g.id = gm.group_id \
           JOIN org_members om ON om.org_id = gm.org_id AND om.user_id = gm.user_id \
                AND om.status = 'active' \
          WHERE g.org_role = 'admin' \
             OR EXISTS (SELECT 1 FROM group_workspace_access b \
                         WHERE b.group_id = g.id AND b.workspace_id = w.id) \
          ORDER BY g.name, g.id",
    )
    .bind(org)
    .bind(user)
    .bind(workspaces)
    .fetch_all(exec)
    .await?)
}

// ---- impact view ----

#[derive(Debug, Clone)]
pub struct ImpactWorkspace {
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub workspace_slug: String,
    pub users_losing_access: Vec<(Uuid, String, String)>,
    pub total: i64,
    pub schedules_disabled: i64,
    pub webhooks_disabled: i64,
    pub api_keys: i64,
}

/// What deleting `group` (or, with `remove_member`, removing that one member) would take away.
/// Per workspace the group reaches (its bindings, or every workspace of the org when it maps to
/// `admin`): the users who would lose **all** access (after counting direct bindings, their org
/// role and every *other* group), the enabled creator-anchored schedules and webhooks that
/// would stop firing, and the live personal API keys confined to the workspace.
pub async fn impact(
    conn: &mut PgConnection,
    group: &GroupRow,
    remove_member: Option<Uuid>,
) -> Result<Vec<ImpactWorkspace>, DbError> {
    let workspaces: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT w.id, w.name, w.slug FROM workspaces w \
          WHERE w.org_id = $1 AND ($2::text = 'admin' OR EXISTS (\
                SELECT 1 FROM group_workspace_access b \
                 WHERE b.group_id = $3 AND b.workspace_id = w.id)) \
          ORDER BY w.name, w.id",
    )
    .bind(group.org_id)
    .bind(group.org_role.as_deref().unwrap_or(""))
    .bind(group.id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for (ws, name, slug) in workspaces {
        // Candidates: active members of the group (or the one member) who have access now through
        // this group and would have none without it.
        let losers: Vec<(Uuid, String, String)> = sqlx::query_as(
            "SELECT u.id, u.email::text, u.display_name \
               FROM group_members gm \
               JOIN org_members om ON om.org_id = gm.org_id AND om.user_id = gm.user_id \
                    AND om.status = 'active' \
               JOIN users u ON u.id = gm.user_id \
              WHERE gm.group_id = $1 AND ($3::uuid IS NULL OR gm.user_id = $3) \
                AND om.role NOT IN ('owner', 'admin') \
                AND NOT EXISTS (SELECT 1 FROM workspace_access x \
                                 WHERE x.workspace_id = $2 AND x.user_id = gm.user_id) \
                AND NOT EXISTS (SELECT 1 FROM group_members g2 \
                                  JOIN groups o ON o.id = g2.group_id \
                                 WHERE g2.org_id = gm.org_id AND g2.user_id = gm.user_id \
                                   AND g2.group_id <> $1 \
                                   AND (o.org_role = 'admin' OR EXISTS ( \
                                        SELECT 1 FROM group_workspace_access b2 \
                                         WHERE b2.group_id = o.id AND b2.workspace_id = $2))) \
              ORDER BY u.display_name, u.id",
        )
        .bind(group.id)
        .bind(ws)
        .bind(remove_member)
        .fetch_all(&mut *conn)
        .await?;
        let ids: Vec<Uuid> = losers.iter().map(|l| l.0).collect();
        let schedules: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM schedules s JOIN agents a ON a.id = s.agent_id \
              WHERE a.workspace_id = $1 AND s.creator_user_id = ANY($2) \
                AND s.anchor = 'creator' AND s.enabled",
        )
        .bind(ws)
        .bind(&ids)
        .fetch_one(&mut *conn)
        .await?;
        let webhooks: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM webhooks k JOIN agents a ON a.id = k.agent_id \
              WHERE a.workspace_id = $1 AND k.creator_user_id = ANY($2) \
                AND k.anchor = 'creator' AND k.enabled",
        )
        .bind(ws)
        .bind(&ids)
        .fetch_one(&mut *conn)
        .await?;
        let keys: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM api_keys \
              WHERE workspace_id = $1 AND user_id = ANY($2) AND kind = 'user' \
                AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())",
        )
        .bind(ws)
        .bind(&ids)
        .fetch_one(&mut *conn)
        .await?;
        let total = losers.len() as i64;
        let mut listed = losers;
        listed.truncate(GROUP_IMPACT_LIST_MAX as usize);
        if total == 0 && schedules == 0 && webhooks == 0 && keys == 0 {
            continue;
        }
        out.push(ImpactWorkspace {
            workspace_id: ws,
            workspace_name: name,
            workspace_slug: slug,
            users_losing_access: listed,
            total,
            schedules_disabled: schedules,
            webhooks_disabled: webhooks,
            api_keys: keys,
        });
    }
    Ok(out)
}

// ---- policy authoring ----

/// Which of `ids` are groups of `org` (a `group` identity must name a group of the scope's org).
pub async fn existing_in_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(
        sqlx::query_scalar("SELECT id FROM groups WHERE org_id = $1 AND id = ANY($2)")
            .bind(org)
            .bind(ids)
            .fetch_all(exec)
            .await?,
    )
}

/// `(id, name, member_count)` for the named groups (names for the editor chips and warnings).
pub async fn names<'e, E: PgExecutor<'e>>(
    exec: E,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, String, Uuid, i64)>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT g.id, g.name::text, g.org_id, \
                (SELECT count(*) FROM group_members m WHERE m.group_id = g.id) \
           FROM groups g WHERE g.id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

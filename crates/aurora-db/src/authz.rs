//! The access law (docs/04 §The access law), implemented once.
//!
//! `can_use(user, ws)`: workspace exists AND user is an *active* org member AND
//! (org owner/admin OR a `workspace_access` row exists). `can_own` additionally
//! needs workspace role `owner` (implicit for org owner/admin). Suspended = none.

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrgRole {
    Member,
    Admin,
    Owner,
}

impl OrgRole {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
    /// Org owners and admins are implicit workspace owners.
    pub fn is_admin(self) -> bool {
        self >= Self::Admin
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberStatus {
    Active,
    Suspended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkspaceRole {
    Member,
    Owner,
}

impl WorkspaceRole {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Self::Owner),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Membership {
    pub role: OrgRole,
    pub status: MemberStatus,
}

impl Membership {
    pub fn is_active(&self) -> bool {
        self.status == MemberStatus::Active
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceAuth {
    pub org_id: Uuid,
    pub org_role: OrgRole,
    pub effective_ws_role: WorkspaceRole,
}

impl WorkspaceAuth {
    /// Anyone holding a `WorkspaceAuth` satisfies `can_use`.
    pub fn can_use(&self) -> bool {
        true
    }
    pub fn can_own(&self) -> bool {
        self.effective_ws_role == WorkspaceRole::Owner
    }
}

/// Pure core of the law for an *active* member: effective workspace role.
pub fn effective_role(org_role: OrgRole, binding: Option<WorkspaceRole>) -> Option<WorkspaceRole> {
    if org_role.is_admin() {
        Some(WorkspaceRole::Owner)
    } else {
        binding
    }
}

fn parse_status(s: &str) -> MemberStatus {
    if s == "active" {
        MemberStatus::Active
    } else {
        MemberStatus::Suspended
    }
}

/// Raw membership (including suspended rows).
pub async fn org_membership<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    org: Uuid,
) -> Result<Option<Membership>, DbError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT role, status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(org)
            .bind(user)
            .fetch_optional(exec)
            .await?;
    Ok(row.and_then(|(r, s)| {
        Some(Membership {
            role: OrgRole::parse(&r)?,
            status: parse_status(&s),
        })
    }))
}

/// Access-law resolution. `None` = workspace missing or caller has no access.
pub async fn workspace_access<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<Option<WorkspaceAuth>, DbError> {
    let row: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT w.org_id, m.role, wa.role
         FROM workspaces w
         JOIN org_members m ON m.org_id = w.org_id AND m.user_id = $1 AND m.status = 'active'
         LEFT JOIN workspace_access wa ON wa.workspace_id = w.id AND wa.user_id = $1
         WHERE w.id = $2",
    )
    .bind(user)
    .bind(ws)
    .fetch_optional(exec)
    .await?;
    Ok(row.and_then(|(org_id, role, binding)| {
        let org_role = OrgRole::parse(&role)?;
        let effective_ws_role =
            effective_role(org_role, binding.as_deref().and_then(WorkspaceRole::parse))?;
        Some(WorkspaceAuth {
            org_id,
            org_role,
            effective_ws_role,
        })
    }))
}

pub async fn can_use<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<bool, DbError> {
    Ok(workspace_access(exec, user, ws).await?.is_some())
}

pub async fn can_own<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<bool, DbError> {
    Ok(workspace_access(exec, user, ws)
        .await?
        .is_some_and(|a| a.can_own()))
}

/// Does `user` hold `can_own` on at least one workspace of `org`? Org owners/admins qualify
/// (they own every workspace implicitly); other active members need a `workspace_access` row
/// with role `owner` on some workspace of the org. Suspended or non-members: `false`.
/// Used for org-scope credential *discovery* (I-27 amendment): a workspace owner may list org
/// secrets/connections to grant them to agents of their workspace.
pub async fn owns_any_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    org: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM org_members m
             WHERE m.org_id = $1 AND m.user_id = $2 AND m.status = 'active'
               AND (m.role IN ('owner','admin')
                    OR EXISTS (SELECT 1 FROM workspace_access a
                               JOIN workspaces w ON w.id = a.workspace_id
                               WHERE w.org_id = $1 AND a.user_id = $2 AND a.role = 'owner')))",
    )
    .bind(org)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

/// Org of a workspace regardless of caller (for key confinement checks).
pub async fn workspace_org<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM workspaces WHERE id = $1")
            .bind(ws)
            .fetch_optional(exec)
            .await?,
    )
}

// ---- conversation visibility (phase 1, RFE-009, D-47) ---------------------------------------

/// `can_see_conversation` (docs/04 §The access law), given the caller's resolved workspace
/// access (which already encodes `can_use`, so a direct user who lost access never gets here).
/// Non-direct kinds: `can_use`. Direct: the direct user, or `can_own`.
pub fn can_see_conversation(
    auth: &WorkspaceAuth,
    user: Uuid,
    kind: &str,
    direct_user_id: Option<Uuid>,
) -> bool {
    kind != "direct" || direct_user_id == Some(user) || auth.can_own()
}

/// Only the direct user writes into a direct conversation (owners read, abort and send-now).
pub fn can_write_conversation(user: Uuid, kind: &str, direct_user_id: Option<Uuid>) -> bool {
    kind != "direct" || direct_user_id == Some(user)
}

/// SQL boolean expression: true when `conv_expr` names a `direct` conversation that `user_param`
/// (a `$n` placeholder) cannot see, *within the workspace `ws_expr`* (clarification C of D-47: an
/// approval in a workspace other than its conversation's is not subject to that conversation's
/// visibility; its conversation reference is redacted instead). A null or unknown conversation is never hidden. Used by every
/// query that lists rows naming a conversation (approvals, activity, ...); keep in step with
/// [`can_see_conversation`].
pub fn hidden_direct_conversation_sql(conv_expr: &str, user_param: &str, ws_expr: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM conversations hc WHERE hc.id = {conv_expr} \
           AND hc.kind = 'direct' AND hc.workspace_id = {ws_expr} \
           AND hc.direct_user_id IS DISTINCT FROM {user_param} \
           AND NOT EXISTS (SELECT 1 FROM workspaces hw \
                JOIN org_members hm ON hm.org_id = hw.org_id AND hm.user_id = {user_param} \
                     AND hm.status = 'active' \
                LEFT JOIN workspace_access hwa ON hwa.workspace_id = hw.id \
                     AND hwa.user_id = {user_param} \
                WHERE hw.id = hc.workspace_id \
                  AND (hm.role IN ('owner', 'admin') OR hwa.role = 'owner')))"
    )
}

/// Can `user` see the conversation `conv` (by id)? Unknown conversation: `false`. Applies
/// `can_use` on its workspace too.
pub async fn can_see_conversation_id<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    conv: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM conversations c \
           JOIN workspaces w ON w.id = c.workspace_id \
           JOIN org_members m ON m.org_id = w.org_id AND m.user_id = $1 AND m.status = 'active' \
           LEFT JOIN workspace_access wa ON wa.workspace_id = w.id AND wa.user_id = $1 \
           WHERE c.id = $2 AND (m.role IN ('owner','admin') OR wa.user_id IS NOT NULL) \
             AND NOT {})",
        hidden_direct_conversation_sql("c.id", "$1", "w.id")
    ))
    .bind(user)
    .bind(conv)
    .fetch_one(exec)
    .await?)
}

/// Which of `ids` name a direct conversation `user` cannot see (to redact references).
pub async fn hidden_conversations<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(&format!(
        "SELECT t.id FROM unnest($2::uuid[]) AS t(id) WHERE {}",
        hidden_direct_conversation_sql("t.id", "$1", "hc.workspace_id")
    ))
    .bind(user)
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// RFE-021: does the workspace have at least one **active** user with `can_own` (workspace
/// `owner` binding, or org `owner`/`admin`)? The fire-time authority of a workspace-anchored
/// schedule or webhook (docs/04 §Fire-time authority). `false` when the workspace is missing.
pub async fn workspace_has_active_owner<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM workspaces w
             JOIN org_members m ON m.org_id = w.org_id AND m.status = 'active'
             WHERE w.id = $1
               AND (m.role IN ('owner','admin')
                    OR EXISTS (SELECT 1 FROM workspace_access a
                               WHERE a.workspace_id = w.id AND a.user_id = m.user_id
                                 AND a.role = 'owner')))",
    )
    .bind(ws)
    .fetch_one(exec)
    .await?)
}

/// RFE-021: fire-time authority of a schedule/webhook. `anchor = workspace`: an active owner
/// exists; otherwise (`creator`) the creator must still hold access to the workspace.
pub async fn automation_authorized(
    conn: &mut sqlx::PgConnection,
    anchor: &str,
    creator: Option<Uuid>,
    ws: Uuid,
) -> Result<bool, DbError> {
    if anchor == "workspace" {
        return workspace_has_active_owner(&mut *conn, ws).await;
    }
    Ok(match creator {
        Some(u) => workspace_access(&mut *conn, u, ws)
            .await?
            .is_some_and(|a| a.can_use()),
        None => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn law() {
        assert_eq!(
            effective_role(OrgRole::Owner, None),
            Some(WorkspaceRole::Owner)
        );
        assert_eq!(
            effective_role(OrgRole::Admin, Some(WorkspaceRole::Member)),
            Some(WorkspaceRole::Owner)
        );
        assert_eq!(effective_role(OrgRole::Member, None), None);
        assert_eq!(
            effective_role(OrgRole::Member, Some(WorkspaceRole::Member)),
            Some(WorkspaceRole::Member)
        );
    }
}

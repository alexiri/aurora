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

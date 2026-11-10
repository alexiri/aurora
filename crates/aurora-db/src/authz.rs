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
    /// Phase 2, RFE-012 (D-55): chats like a member and may decide approvals.
    Approver,
    Owner,
}

impl WorkspaceRole {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Self::Owner),
            "approver" => Some(Self::Approver),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Approver => "approver",
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
    /// Phase 2, RFE-012 (D-55): workspace `owner` or `approver` (org owner/admin are implicit
    /// owners). `can_own ⊂ can_approve ⊂ can_use`.
    pub fn can_approve(&self) -> bool {
        self.effective_ws_role >= WorkspaceRole::Approver
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

/// Why an effective role exists (phase 4, P4-GRP-1, D-130). Informational only: sources explain
/// the result to the dashboard and are never an input to a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessSourceKind {
    /// A direct `workspace_access` binding (a manual one).
    Direct,
    /// A binding created or raised by an IdP role claim (`idp:<provider>:<value>`, P4-IDP-1).
    IdpClaim,
    /// A workspace binding of a group the user belongs to.
    Group,
    /// The user's own org role (`admin`/`owner`): implicit workspace owner.
    OrgRole,
    /// An `admin` org-role mapping of a group the user belongs to: implicit workspace owner.
    OrgRoleViaGroup,
}

impl AccessSourceKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::IdpClaim => "idp_claim",
            Self::Group => "group",
            Self::OrgRole => "org_role",
            Self::OrgRoleViaGroup => "org_role_via_group",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessSource {
    pub kind: AccessSourceKind,
    /// The group, for `Group` and `OrgRoleViaGroup`.
    pub group: Option<Uuid>,
    /// The claim source (`idp:<provider>:<value>`), for `IdpClaim`.
    pub value: Option<String>,
}

/// The effective result of the access law for one (user, workspace) with its sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveAccess {
    pub auth: WorkspaceAuth,
    pub sources: Vec<AccessSource>,
}

/// `effective_access(user, workspace)` (docs/04 §Groups, D-130(b)): **the** access-law query.
/// One statement: the roles come from the `effective_workspace_access` / `effective_org_members`
/// views (the strongest of direct, group and IdP-claim sources, active members only) and the
/// sources from the same rows. `None` = workspace missing or the caller has no access.
pub async fn effective_access<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<Option<EffectiveAccess>, DbError> {
    type Row = (
        Uuid,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        serde_json::Value,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT ew.org_id, eo.role, ew.role, m.role, wa.role, wa.idp_source, wa.base_role,
                COALESCE((SELECT jsonb_agg(jsonb_build_object(
                              'group', g.id, 'org_role', g.org_role, 'ws_role', b.role))
                            FROM group_members gm
                            JOIN groups g ON g.id = gm.group_id
                            LEFT JOIN group_workspace_access b
                                   ON b.group_id = g.id AND b.workspace_id = ew.workspace_id
                           WHERE gm.org_id = ew.org_id AND gm.user_id = ew.user_id
                             AND (g.org_role IS NOT NULL OR b.role IS NOT NULL)),
                         '[]'::jsonb)
           FROM effective_workspace_access ew
           JOIN effective_org_members eo ON eo.org_id = ew.org_id AND eo.user_id = ew.user_id
           JOIN org_members m ON m.org_id = ew.org_id AND m.user_id = ew.user_id
           LEFT JOIN workspace_access wa
                  ON wa.workspace_id = ew.workspace_id AND wa.user_id = ew.user_id
          WHERE ew.workspace_id = $2 AND ew.user_id = $1",
    )
    .bind(user)
    .bind(ws)
    .fetch_optional(exec)
    .await?;
    Ok(row.and_then(
        |(org_id, org_role, ws_role, direct_org, direct_ws, idp_source, base_role, groups)| {
            let auth = WorkspaceAuth {
                org_id,
                org_role: OrgRole::parse(&org_role)?,
                effective_ws_role: WorkspaceRole::parse(&ws_role)?,
            };
            let sources = build_sources(
                &direct_org,
                direct_ws.as_deref(),
                idp_source,
                base_role.as_deref(),
                &groups,
            );
            Some(EffectiveAccess { auth, sources })
        },
    ))
}

/// The explanation of an effective role (informational; never an input to a decision).
/// `groups` is the jsonb array `[{group, org_role, ws_role}]` of the user's contributing groups.
pub fn build_sources(
    direct_org: &str,
    direct_ws: Option<&str>,
    idp_source: Option<String>,
    base_role: Option<&str>,
    groups: &serde_json::Value,
) -> Vec<AccessSource> {
    let mut sources = Vec::new();
    if OrgRole::parse(direct_org).is_some_and(OrgRole::is_admin) {
        sources.push(AccessSource {
            kind: AccessSourceKind::OrgRole,
            group: None,
            value: None,
        });
    }
    if direct_ws.is_some() {
        let claim = idp_source.is_some();
        if let Some(v) = idp_source {
            sources.push(AccessSource {
                kind: AccessSourceKind::IdpClaim,
                group: None,
                value: Some(v),
            });
        }
        if base_role.is_some() || !claim {
            sources.push(AccessSource {
                kind: AccessSourceKind::Direct,
                group: None,
                value: None,
            });
        }
    }
    for g in groups.as_array().into_iter().flatten() {
        let id = g
            .get("group")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());
        if g.get("org_role").and_then(|v| v.as_str()) == Some("admin") {
            sources.push(AccessSource {
                kind: AccessSourceKind::OrgRoleViaGroup,
                group: id,
                value: None,
            });
        }
        if g.get("ws_role").is_some_and(|v| !v.is_null()) {
            sources.push(AccessSource {
                kind: AccessSourceKind::Group,
                group: id,
                value: None,
            });
        }
    }
    sources
}

/// Access-law resolution. `None` = workspace missing or caller has no access.
pub async fn workspace_access<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<Option<WorkspaceAuth>, DbError> {
    Ok(effective_access(exec, user, ws).await?.map(|e| e.auth))
}

/// The user's **effective org role** in `org` (the strongest of `org_members.role` and the
/// `org_role` of the user's groups), only while the membership is `active`; `None` otherwise.
pub async fn effective_org_role<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    org: Uuid,
) -> Result<Option<OrgRole>, DbError> {
    let r: Option<String> = sqlx::query_scalar(
        "SELECT role FROM effective_org_members WHERE org_id = $1 AND user_id = $2",
    )
    .bind(org)
    .bind(user)
    .fetch_optional(exec)
    .await?;
    Ok(r.as_deref().and_then(OrgRole::parse))
}

/// Pure statement of the effective-role rule for tests and documentation: the strongest of the
/// direct and group-derived values, never a sum, never lower than the direct one; org owners and
/// admins (effective) are implicit workspace owners.
pub fn combine_roles(
    direct_org: OrgRole,
    group_org_roles: &[OrgRole],
    direct_ws: Option<WorkspaceRole>,
    group_ws_roles: &[WorkspaceRole],
) -> (OrgRole, Option<WorkspaceRole>) {
    let org = group_org_roles
        .iter()
        .copied()
        .fold(direct_org, OrgRole::max);
    let ws = if org.is_admin() {
        Some(WorkspaceRole::Owner)
    } else {
        group_ws_roles.iter().copied().chain(direct_ws).max()
    };
    (org, ws)
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

/// `can_approve` (phase 2, RFE-012, D-55): workspace `owner` or `approver`, or org owner/admin.
pub async fn can_approve<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<bool, DbError> {
    Ok(workspace_access(exec, user, ws)
        .await?
        .is_some_and(|a| a.can_approve()))
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
             SELECT 1 FROM effective_org_members m
             WHERE m.org_id = $1 AND m.user_id = $2
               AND (m.role IN ('owner','admin')
                    OR EXISTS (SELECT 1 FROM effective_workspace_access a
                               WHERE a.org_id = $1 AND a.user_id = $2 AND a.role = 'owner')))",
    )
    .bind(org)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

/// (P4-GSL) `can_use` on at least one workspace of the org: an active org owner/admin, or an
/// active member with a workspace binding in the org. Suspended or non-members: `false`.
pub async fn uses_any_workspace<'e, E: PgExecutor<'e>>(
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
                               WHERE w.org_id = $1 AND a.user_id = $2)))",
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
           AND NOT EXISTS (SELECT 1 FROM effective_workspace_access hx \
                WHERE hx.workspace_id = hc.workspace_id AND hx.user_id = {user_param} \
                  AND hx.role = 'owner'))"
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
           JOIN effective_workspace_access ea ON ea.workspace_id = w.id AND ea.user_id = $1 \
           WHERE c.id = $2 AND NOT {})",
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
             SELECT 1 FROM effective_workspace_access a
             WHERE a.workspace_id = $1 AND a.role = 'owner')",
    )
    .bind(ws)
    .fetch_one(exec)
    .await?)
}

/// RFE-021: fire-time authority of a schedule/webhook. `anchor = workspace`: an active owner
/// exists; otherwise (`creator`) the creator must still hold access to the workspace.
///
/// This is *authority* only. A frozen agent (RFE-006, FU-P2-021a) is not an authority failure — the
/// automation stays enabled — so callers check [`crate::repo::freeze::is_agent_frozen`] separately
/// and, for both anchors alike, skip the fire (schedules) or answer 410 `frozen` (webhooks).
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
            effective_role(OrgRole::Member, Some(WorkspaceRole::Approver)),
            Some(WorkspaceRole::Approver)
        );
        assert_eq!(
            effective_role(OrgRole::Member, Some(WorkspaceRole::Member)),
            Some(WorkspaceRole::Member)
        );
    }

    fn auth(r: WorkspaceRole) -> WorkspaceAuth {
        WorkspaceAuth {
            org_id: Uuid::nil(),
            org_role: OrgRole::Member,
            effective_ws_role: r,
        }
    }

    #[test]
    fn approver_hierarchy() {
        let (m, a, o) = (
            auth(WorkspaceRole::Member),
            auth(WorkspaceRole::Approver),
            auth(WorkspaceRole::Owner),
        );
        assert!(m.can_use() && a.can_use() && o.can_use());
        assert!(!m.can_approve() && a.can_approve() && o.can_approve());
        assert!(!m.can_own() && !a.can_own() && o.can_own());
        assert_eq!(
            WorkspaceRole::parse("approver"),
            Some(WorkspaceRole::Approver)
        );
        assert!(WorkspaceRole::Member < WorkspaceRole::Approver);
    }
}

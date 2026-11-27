//! The access law (docs/04 §The access law), implemented once.
//!
//! `can_use(user, ws)`: workspace exists AND user is an *active* org member AND holds access
//! **in their own right** (an active direct or group binding; D-473, D-477). `can_own`
//! additionally needs workspace role `owner`. An organization role (owner/admin) grants no
//! workspace right; it gives `can_administer` (C-1160) instead. Suspended = none.

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
    /// Runs the organization: org-level routes (D-477). The org role grants **no** workspace
    /// right; for the narrow workspace right see [`can_administer`].
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
    /// (P5-UX-2) The workspace the access was resolved for: a refusal names who can help in it.
    pub workspace_id: Uuid,
    pub org_role: OrgRole,
    pub effective_ws_role: WorkspaceRole,
}

impl WorkspaceAuth {
    // (FIX-181 F10) No `can_use()`: a `WorkspaceAuth` is only ever built for an active member with
    // an effective workspace role, so holding one *is* `can_use`; the check lives in the
    // construction (`workspace_access`).
    pub fn can_own(&self) -> bool {
        self.effective_ws_role == WorkspaceRole::Owner
    }
    /// Phase 2, RFE-012 (D-55): workspace `owner` or `approver` in their own right (D-477: the
    /// org role is not an implicit owner). `can_own ⊂ can_approve ⊂ can_use`.
    pub fn can_approve(&self) -> bool {
        self.effective_ws_role >= WorkspaceRole::Approver
    }
}

/// Pure core of the law for an *active* member: the effective workspace role is the person's own
/// binding (D-477: the org role grants no workspace right).
pub fn effective_role(binding: Option<WorkspaceRole>) -> Option<WorkspaceRole> {
    binding
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
}

impl AccessSourceKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::IdpClaim => "idp_claim",
            Self::Group => "group",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessSource {
    pub kind: AccessSourceKind,
    /// The group, for `Group`.
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
        |(org_id, org_role, ws_role, _direct_org, direct_ws, idp_source, base_role, groups)| {
            let auth = WorkspaceAuth {
                org_id,
                workspace_id: ws,
                org_role: OrgRole::parse(&org_role)?,
                effective_ws_role: WorkspaceRole::parse(&ws_role)?,
            };
            let sources = build_sources(
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
/// Only sources that hold **in the person's own right** appear (D-477): an org role or an
/// `admin` group mapping is not a workspace-access source.
pub fn build_sources(
    direct_ws: Option<&str>,
    idp_source: Option<String>,
    base_role: Option<&str>,
    groups: &serde_json::Value,
) -> Vec<AccessSource> {
    let mut sources = Vec::new();
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
/// direct and group-derived values, never a sum, never lower than the direct one. The org role
/// grants no workspace right (D-477): the workspace role is the strongest own binding only.
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
    let ws = group_ws_roles.iter().copied().chain(direct_ws).max();
    (org, ws)
}

pub async fn can_use<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<bool, DbError> {
    Ok(workspace_access(exec, user, ws).await?.is_some())
}

/// (FIX-230, D-473(a), C-1157) The person's access to `ws` **in their own right**: an active direct
/// or group binding while an active org member and not suspended there (`own_workspace_access`).
/// Chat offers and auto-picks agents by this; since FIX-234 (D-477) authorization (`can_use`,
/// `can_own`) rests on the same access, and no org role confers any. Reads people tables through
/// the view: call it from the API only, never from a function the gateway runs.
pub async fn own_access<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<Option<WorkspaceRole>, DbError> {
    let r: Option<String> = sqlx::query_scalar(
        "SELECT role FROM own_workspace_access WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(user)
    .fetch_optional(exec)
    .await?;
    Ok(r.as_deref().and_then(WorkspaceRole::parse))
}

/// The outcome of [`can_ssh`] (docs/04 §SSH access, D-239(a)): who may SSH into which sandbox is
/// the web terminal's rule. `Denied` collapses "agent unknown", "person disabled" and "no
/// `can_use`" so a caller cannot probe which one it was (the API answers 404).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshAccess {
    /// `ssh.enabled`, the person is active, `can_use` holds, the agent is hosted and not frozen.
    Allowed { workspace_id: Uuid, org_id: Uuid },
    /// `ssh.enabled` is off.
    Disabled,
    /// The agent does not exist, the person is disabled or has no `can_use` of its workspace.
    Denied,
    /// A BYO agent: it has no sandbox to reach (422 `ssh_not_supported`).
    NotHosted,
    /// A freeze applies to the agent, its workspace or its org (423 `frozen`; the level).
    Frozen(crate::repo::freeze::Scope),
}

/// `can_ssh(user, agent)` (docs/04 §SSH access): `ssh_enabled` **and** the user is not disabled
/// **and** `can_use` of the agent's workspace **and** the agent is hosted **and** no freeze
/// applies. Implemented once here and evaluated at the certificate mint, the session open, every
/// channel open and every heartbeat. The order of the refusals is the terminal's (`Denied`
/// before `NotHosted` before `Frozen`), so a person without access learns nothing.
pub async fn can_ssh(
    conn: &mut sqlx::PgConnection,
    ssh_enabled: bool,
    user: Uuid,
    agent: Uuid,
) -> Result<SshAccess, DbError> {
    if !ssh_enabled {
        return Ok(SshAccess::Disabled);
    }
    let row: Option<(Uuid, Uuid, String, bool)> = sqlx::query_as(
        "SELECT a.workspace_id, a.org_id, a.kind,
                EXISTS (SELECT 1 FROM users u WHERE u.id = $2 AND u.disabled_at IS NULL)
         FROM agents a WHERE a.id = $1",
    )
    .bind(agent)
    .bind(user)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((workspace_id, org_id, kind, user_active)) = row else {
        return Ok(SshAccess::Denied);
    };
    if !user_active || !can_use(&mut *conn, user, workspace_id).await? {
        return Ok(SshAccess::Denied);
    }
    if kind != "hosted" {
        return Ok(SshAccess::NotHosted);
    }
    if let Some(e) = crate::repo::freeze::effective_of_agent(conn, agent).await? {
        return Ok(SshAccess::Frozen(e.level));
    }
    Ok(SshAccess::Allowed {
        workspace_id,
        org_id,
    })
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

/// `can_approve` (phase 2, RFE-012, D-55): workspace `owner` or `approver` in their own right
/// (D-477: an org role confers no workspace right).
pub async fn can_approve<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<bool, DbError> {
    Ok(workspace_access(exec, user, ws)
        .await?
        .is_some_and(|a| a.can_approve()))
}

/// Does `user` hold `can_own` on at least one workspace of `org` **in their own right**?
/// (D-477: an org role grants no workspace right, so org owners/admins no longer qualify
/// by their role.) Used for org-scope credential *discovery* (I-27 amendment): a workspace
/// owner may list org secrets/connections to grant them to agents of their workspace.
pub async fn owns_any_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    org: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM effective_workspace_access a
                        WHERE a.org_id = $1 AND a.user_id = $2 AND a.role = 'owner')",
    )
    .bind(org)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

/// (P4-GSL) `can_use` on at least one workspace of the org: an active member with workspace
/// access in the org **in their own right**, direct or through a group (the
/// `effective_workspace_access` view, C-469, D-477). Suspended or non-members: `false`.
pub async fn uses_any_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    org: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM effective_workspace_access a
                        WHERE a.org_id = $1 AND a.user_id = $2)",
    )
    .bind(org)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

/// (FIX-234, D-477(b), C-1160) The narrow right an organization role keeps on a workspace:
/// true when the user holds own-right `owner` there, or is an active effective org owner or
/// admin of its organization. It covers seeing the workspace's metadata, managing its people,
/// freezing and deleting it, offboarding/reassignment, and the organization's own accounts —
/// it never reads content, starts agent work, decides an approval or touches the workspace's
/// own credentials, grants or policy.
pub async fn can_administer<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM workspaces w
             JOIN effective_org_members m ON m.org_id = w.org_id AND m.user_id = $2
              AND m.role IN ('owner', 'admin')
             WHERE w.id = $1)
          OR EXISTS (SELECT 1 FROM own_workspace_access a
                     WHERE a.workspace_id = $1 AND a.user_id = $2 AND a.role = 'owner')",
    )
    .bind(ws)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

/// (FIX-236, D-478, C-1164) One active emergency grant, as [`emergency_reader`] resolved it.
/// Constructed only inside this module: no `From`, no public constructor — an
/// `EmergencyGrant` can never become a [`WorkspaceAuth`] or an `AdminAuth` (C-1165), so it can
/// never satisfy `can_use`, `can_approve`, `can_own`, `can_administer` or `can_ssh`, and no
/// write route accepts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmergencyGrant {
    id: Uuid,
    org_id: Uuid,
    workspace_id: Uuid,
    /// The opener of the grant (the reader while it is theirs).
    user_id: Uuid,
    reason: String,
    opened_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
    expires_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
}

impl EmergencyGrant {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn org_id(&self) -> Uuid {
        self.org_id
    }
    pub fn workspace_id(&self) -> Uuid {
        self.workspace_id
    }
    pub fn user_id(&self) -> Uuid {
        self.user_id
    }
    pub fn reason(&self) -> &str {
        &self.reason
    }
    pub fn opened_at(&self) -> sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc> {
        self.opened_at
    }
    pub fn expires_at(&self) -> sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc> {
        self.expires_at
    }
}

/// The read authority a read route accepts (FIX-236, C-1165): the caller's own access, or an
/// active emergency grant. Writes never take it — they keep [`WorkspaceAuth`], which an
/// `Emergency` cannot produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadAuth {
    Own(WorkspaceAuth),
    Emergency(EmergencyGrant),
}

impl ReadAuth {
    pub fn workspace_id(&self) -> Uuid {
        match self {
            Self::Own(a) => a.workspace_id,
            Self::Emergency(g) => g.workspace_id(),
        }
    }
    pub fn org_id(&self) -> Uuid {
        match self {
            Self::Own(a) => a.org_id,
            Self::Emergency(g) => g.org_id(),
        }
    }
    /// `can_see_conversation` through a `ReadAuth` (D-478(b): the emergency reader reads what an
    /// owner reads — every conversation of the workspace, direct ones included). Own access
    /// keeps its own rule.
    pub fn can_see_conversation(
        &self,
        user: Uuid,
        kind: &str,
        direct_user_id: Option<Uuid>,
    ) -> bool {
        match self {
            Self::Own(a) => can_see_conversation(a, user, kind, direct_user_id),
            Self::Emergency(_) => true,
        }
    }
    /// The caller's own access, when the read authority is their own (an emergency grant is
    /// never a `WorkspaceAuth`).
    pub fn own(&self) -> Option<&WorkspaceAuth> {
        match self {
            Self::Own(a) => Some(a),
            Self::Emergency(_) => None,
        }
    }
    /// Holding a `WorkspaceAuth` *is* `can_use` (see its impl); an emergency grant reads like a
    /// member. Lists and metadata need nothing more than the `ReadAuth` itself.
    /// `can_own` for the read routes (C-1165, D-478(b)): the emergency reader reads what an
    /// owner reads — every conversation, direct ones included, and every message in search.
    /// Never usable for a write: writes resolve [`WorkspaceAuth`] through [`Caller::workspace`].
    pub fn can_own(&self) -> bool {
        match self {
            Self::Own(a) => a.can_own(),
            Self::Emergency(_) => true,
        }
    }
}

/// (FIX-236, C-1165) The one helper. `Some` only while `user` holds an **active** emergency
/// grant on `ws` (`ended_at IS NULL AND expires_at > now()`, evaluated here on every call —
/// never cached) **and** is still an active effective org owner of its organization (a lowered,
/// suspended or removed owner loses the grant at once, even before the access-lost hook ran).
pub async fn emergency_reader<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ws: Uuid,
) -> Result<Option<EmergencyGrant>, DbError> {
    let row: Option<(Uuid, Uuid, Uuid, String, _, _)> = sqlx::query_as(
        "SELECT e.id, e.org_id, e.workspace_id, e.reason, e.opened_at, e.expires_at \
         FROM workspace_emergency_access e \
         JOIN effective_org_members m ON m.org_id = e.org_id AND m.user_id = e.user_id \
                                      AND m.role = 'owner' \
        WHERE e.workspace_id = $1 AND e.user_id = $2 \
          AND e.ended_at IS NULL AND e.expires_at > now()",
    )
    .bind(ws)
    .bind(user)
    .fetch_optional(exec)
    .await?;
    Ok(row.map(
        |(id, org_id, workspace_id, reason, opened_at, expires_at)| EmergencyGrant {
            id,
            org_id,
            workspace_id,
            user_id: user,
            reason,
            opened_at,
            expires_at,
        },
    ))
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
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM conversations c \
           JOIN workspaces w ON w.id = c.workspace_id \
           JOIN effective_workspace_access ea ON ea.workspace_id = w.id AND ea.user_id = $1 \
           WHERE c.id = $2 AND NOT {})",
        hidden_direct_conversation_sql("c.id", "$1", "w.id")
    )))
    .bind(user)
    .bind(conv)
    .fetch_one(exec)
    .await?)
}

/// (FIX-236, C-1165) [`can_see_conversation_id`] extended with the emergency grant: a caller
/// holding an **active** grant on the conversation's workspace (and still an active effective
/// org owner) reads it like an owner — the read stream's resync checks use this, so a stream
/// opened under emergency access ends when the grant ends. Keep in step with
/// [`emergency_reader`].
pub async fn can_see_conversation_id_emergency<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    conv: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM conversations c \
           JOIN workspaces w ON w.id = c.workspace_id \
           JOIN effective_workspace_access ea ON ea.workspace_id = w.id AND ea.user_id = $1 \
           WHERE c.id = $2 AND NOT {HC}) \
         OR EXISTS (SELECT 1 FROM conversations c2 \
           JOIN workspace_emergency_access e ON e.workspace_id = c2.workspace_id \
                AND e.user_id = $1 AND e.ended_at IS NULL AND e.expires_at > now() \
           JOIN effective_org_members m ON m.org_id = e.org_id AND m.user_id = e.user_id \
                AND m.role = 'owner' \
           WHERE c2.id = $2)",
        HC = hidden_direct_conversation_sql("c.id", "$1", "w.id")
    )))
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
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT t.id FROM unnest($2::uuid[]) AS t(id) WHERE {}",
        hidden_direct_conversation_sql("t.id", "$1", "hc.workspace_id")
    )))
    .bind(user)
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// RFE-021: does the workspace have at least one **active** user with `can_own` (an `owner`
/// binding in their own right, direct or through a group; D-477: no org role counts)? The fire-time authority of a workspace-anchored
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
        Some(u) => workspace_access(&mut *conn, u, ws).await?.is_some(),
        None => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn law() {
        assert_eq!(effective_role(None), None);
        assert_eq!(
            effective_role(Some(WorkspaceRole::Approver)),
            Some(WorkspaceRole::Approver)
        );
        assert_eq!(
            effective_role(Some(WorkspaceRole::Member)),
            Some(WorkspaceRole::Member)
        );
        assert_eq!(
            effective_role(Some(WorkspaceRole::Owner)),
            Some(WorkspaceRole::Owner)
        );
    }

    fn auth(r: WorkspaceRole) -> WorkspaceAuth {
        WorkspaceAuth {
            org_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
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
        assert!(!m.can_approve() && a.can_approve() && o.can_approve());
        assert!(!m.can_own() && !a.can_own() && o.can_own());
        assert_eq!(
            WorkspaceRole::parse("approver"),
            Some(WorkspaceRole::Approver)
        );
        assert!(WorkspaceRole::Member < WorkspaceRole::Approver);
    }
}

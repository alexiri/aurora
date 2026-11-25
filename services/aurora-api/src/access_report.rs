//! (FIX-234, D-477, C-1163) The access report: a read-only CLI subcommand (`aurora-api
//! access-report`) that lists what stops working when the implicit org-role access goes away.
//! Run it before upgrading; whatever it lists needs its people added to their workspaces first
//! (`15`). It writes nothing and prints ids and names only — never content, never a secret.
//!
//! Per organization it lists:
//! 1. workspaces with **no active own-right owner**, marking personal ones with the person's
//!    role there;
//! 2. enabled schedules and webhooks whose fire-time authority will fail: a `creator` anchor
//!    whose creator has no own binding, and a `workspace` anchor in a workspace from item 1;
//! 3. channel objects — space bindings, DM defaults, direct thread links — that tie a person to
//!    an agent of a workspace that person has no own access to;
//! 4. live `auo_` org keys whose creator, an effective org owner/admin, has no own access to at
//!    least one shared workspace of the org;
//! 5. open approvals in workspaces with no own-right decider (`approver`/`owner` binding).

use crate::app::AppState;
use aurora_core::ApiError;
use aurora_db::DbError;
use serde::Serialize;
use uuid::Uuid;

/// A workspace with no active own-right owner (item 1).
#[derive(Debug, Serialize)]
pub struct OwnerlessWorkspace {
    pub workspace_id: Uuid,
    pub name: String,
    pub slug: String,
    /// The person of a personal workspace, with their binding role there (`null` = no binding).
    pub personal: Option<PersonalPerson>,
}

#[derive(Debug, Serialize)]
pub struct PersonalPerson {
    pub user_id: Uuid,
    pub display_name: String,
    pub role: Option<String>,
}

/// A schedule or webhook whose fire-time authority will fail (item 2).
#[derive(Debug, Serialize)]
pub struct FailingAutomation {
    pub kind: &'static str,
    pub id: Uuid,
    pub name: String,
    pub agent_id: Uuid,
    pub agent_name: String,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub anchor: String,
    /// `creator` anchor: the creator (their name when known).
    pub creator: Option<(Uuid, String)>,
}

/// A channel object tying a person to an agent of a workspace they have no own access to
/// (item 3).
#[derive(Debug, Serialize)]
pub struct ChannelTie {
    pub kind: &'static str,
    pub label: String,
    pub agent_id: Uuid,
    pub agent_name: String,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub user_id: Uuid,
    pub display_name: String,
}

/// A live `auo_` key whose creator will lose reach (item 4).
#[derive(Debug, Serialize)]
pub struct OrgKey {
    pub key_id: Uuid,
    pub prefix: String,
    pub creator_id: Uuid,
    pub creator_name: String,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// An open approval with no own-right decider (item 5).
#[derive(Debug, Serialize)]
pub struct UndecidableApproval {
    pub approval_id: Uuid,
    pub agent_id: Uuid,
    pub agent_name: String,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub deadline: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct OrgReport {
    pub org_id: Uuid,
    pub org_slug: String,
    pub org_name: String,
    pub ownerless: Vec<OwnerlessWorkspace>,
    pub failing_automations: Vec<FailingAutomation>,
    pub channel_ties: Vec<ChannelTie>,
    pub org_keys: Vec<OrgKey>,
    pub undecidable: Vec<UndecidableApproval>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub orgs: Vec<OrgReport>,
}

/// The raw rows of the item queries: tall enough that clippy wants a name for them.
type OwnerlessRow = (
    Uuid,
    String,
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<String>,
    Option<String>,
);
type AutomationRow = (
    Uuid,
    String,
    Uuid,
    String,
    Uuid,
    String,
    String,
    Option<Uuid>,
    Option<String>,
);
type OrgKeyRow = (
    Uuid,
    String,
    Uuid,
    String,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// The workspaces of `org` with no active own-right owner (item 1), with the person of a
/// personal workspace and their binding role.
async fn ownerless(
    conn: &mut sqlx::PgConnection,
    org: Uuid,
) -> Result<Vec<OwnerlessWorkspace>, ApiError> {
    let rows: Vec<OwnerlessRow> = sqlx::query_as(
        "SELECT w.id, w.name, w.slug, w.personal_owner_user_id, p.id, p.display_name, \
             (SELECT o.role FROM own_workspace_access o \
              WHERE o.workspace_id = w.id AND o.user_id = w.personal_owner_user_id) \
             FROM workspaces w \
             LEFT JOIN users p ON p.id = w.personal_owner_user_id \
             WHERE w.org_id = $1 \
               AND NOT EXISTS (SELECT 1 FROM own_workspace_access x \
                               WHERE x.workspace_id = w.id AND x.role = 'owner') \
             ORDER BY w.slug",
    )
    .bind(org)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, slug, personal_owner, user_id, display_name, role)| OwnerlessWorkspace {
                workspace_id: id,
                name,
                slug,
                personal: personal_owner.map(|_| PersonalPerson {
                    user_id: user_id.unwrap_or_default(),
                    display_name: display_name.unwrap_or_default(),
                    role,
                }),
            },
        )
        .collect())
}

/// Enabled schedules and webhooks whose fire-time authority will fail (item 2).
async fn failing_automations(
    conn: &mut sqlx::PgConnection,
    org: Uuid,
) -> Result<Vec<FailingAutomation>, ApiError> {
    // `creator` anchor: the creator has no binding at all in the agent's workspace (no own
    // `can_use`). A null creator lacks `can_use` trivially. `workspace` anchor: the workspace
    // has no active own-right owner.
    let schedules: Vec<AutomationRow> = sqlx::query_as(
        "SELECT s.id, s.name, a.id, a.name, w.id, w.name, s.anchor, s.creator_user_id, \
             u.display_name \
             FROM schedules s \
             JOIN agents a ON a.id = s.agent_id \
             JOIN workspaces w ON w.id = a.workspace_id \
             LEFT JOIN users u ON u.id = s.creator_user_id \
             WHERE w.org_id = $1 AND s.enabled \
               AND (s.anchor = 'creator' \
                    AND NOT EXISTS (SELECT 1 FROM own_workspace_access o \
                                    WHERE o.workspace_id = w.id AND o.user_id = s.creator_user_id) \
                 OR s.anchor = 'workspace' \
                    AND NOT EXISTS (SELECT 1 FROM own_workspace_access o \
                                    WHERE o.workspace_id = w.id AND o.role = 'owner')) \
             ORDER BY s.name",
    )
    .bind(org)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let webhooks: Vec<AutomationRow> = sqlx::query_as(
        "SELECT v.id, v.name, a.id, a.name, w.id, w.name, v.anchor, v.creator_user_id, \
             u.display_name \
             FROM webhooks v \
             JOIN agents a ON a.id = v.agent_id \
             JOIN workspaces w ON w.id = a.workspace_id \
             LEFT JOIN users u ON u.id = v.creator_user_id \
             WHERE w.org_id = $1 AND v.enabled \
               AND (v.anchor = 'creator' \
                    AND NOT EXISTS (SELECT 1 FROM own_workspace_access o \
                                    WHERE o.workspace_id = w.id AND o.user_id = v.creator_user_id) \
                 OR v.anchor = 'workspace' \
                    AND NOT EXISTS (SELECT 1 FROM own_workspace_access o \
                                    WHERE o.workspace_id = w.id AND o.role = 'owner')) \
             ORDER BY v.name",
    )
    .bind(org)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let mut out: Vec<FailingAutomation> = schedules
        .into_iter()
        .map(
            |(id, name, agent_id, agent_name, ws, ws_name, anchor, creator, creator_name)| {
                FailingAutomation {
                    kind: "schedule",
                    id,
                    name,
                    agent_id,
                    agent_name,
                    workspace_id: ws,
                    workspace_name: ws_name,
                    anchor,
                    creator: creator.map(|u| (u, creator_name.unwrap_or_default())),
                }
            },
        )
        .collect();
    out.extend(webhooks.into_iter().map(
        |(id, name, agent_id, agent_name, ws, ws_name, anchor, creator, creator_name)| {
            FailingAutomation {
                kind: "webhook",
                id,
                name,
                agent_id,
                agent_name,
                workspace_id: ws,
                workspace_name: ws_name,
                anchor,
                creator: creator.map(|u| (u, creator_name.unwrap_or_default())),
            }
        },
    ));
    Ok(out)
}

/// Channel objects that tie a person to an agent of a workspace they have no own access to
/// (item 3): space bindings (`bound_by`), DM defaults and direct thread links (the linked
/// person, via `user_links`).
async fn channel_ties(
    conn: &mut sqlx::PgConnection,
    org: Uuid,
) -> Result<Vec<ChannelTie>, ApiError> {
    let bindings: Vec<(String, Uuid, String, Uuid, String, Uuid, String)> = sqlx::query_as(
        "SELECT b.space_label, a.id, a.name, w.id, w.name, p.id, p.display_name \
         FROM space_bindings b \
         JOIN agents a ON a.id = b.agent_id \
         JOIN workspaces w ON w.id = a.workspace_id \
         JOIN channel_integrations i ON i.id = b.integration_id \
         JOIN users p ON p.id = b.bound_by_user_id \
             WHERE i.org_id = $1 \
               AND NOT EXISTS (SELECT 1 FROM own_workspace_access o \
                               WHERE o.workspace_id = w.id AND o.user_id = b.bound_by_user_id)",
    )
    .bind(org)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let dms: Vec<(String, Uuid, String, Uuid, String, Uuid, String)> = sqlx::query_as(
        "SELECT coalesce(l.handle, d.platform_user_ref), a.id, a.name, w.id, w.name, \
         p.id, p.display_name \
         FROM dm_defaults d \
         JOIN agents a ON a.id = d.agent_id \
         JOIN workspaces w ON w.id = a.workspace_id \
         JOIN channel_integrations i ON i.id = d.integration_id \
         JOIN user_links l ON l.integration_id = i.id \
              AND l.platform_user_ref = d.platform_user_ref \
         JOIN users p ON p.id = l.user_id \
         WHERE i.org_id = $1 \
           AND NOT EXISTS (SELECT 1 FROM own_workspace_access o \
                           WHERE o.workspace_id = w.id AND o.user_id = p.id)",
    )
    .bind(org)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let threads: Vec<(String, Uuid, String, Uuid, String, Uuid, String)> = sqlx::query_as(
        "SELECT t.thread_ref, a.id, a.name, w.id, w.name, p.id, p.display_name \
         FROM thread_links t \
         JOIN agents a ON a.id = t.agent_id \
         JOIN workspaces w ON w.id = a.workspace_id \
         JOIN channel_integrations i ON i.id = t.integration_id \
         JOIN channel_presences cp ON cp.id = (t.sender->>'presence_id')::uuid \
              AND cp.integration_id = t.integration_id \
         JOIN user_links l ON l.integration_id = i.id \
              AND l.platform_user_ref = cp.platform_user_ref \
         JOIN users p ON p.id = l.user_id \
         WHERE i.org_id = $1 AND t.kind = 'direct' \
           AND t.sender->>'mode' = 'dedicated' \
           AND NOT EXISTS (SELECT 1 FROM own_workspace_access o \
                           WHERE o.workspace_id = w.id AND o.user_id = p.id)",
    )
    .bind(org)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let mut out: Vec<ChannelTie> = bindings
        .into_iter()
        .map(
            |(label, agent_id, agent_name, ws, ws_name, user_id, display_name)| ChannelTie {
                kind: "space_binding",
                label,
                agent_id,
                agent_name,
                workspace_id: ws,
                workspace_name: ws_name,
                user_id,
                display_name,
            },
        )
        .collect();
    out.extend(dms.into_iter().map(
        |(label, agent_id, agent_name, ws, ws_name, user_id, display_name)| ChannelTie {
            kind: "dm_default",
            label,
            agent_id,
            agent_name,
            workspace_id: ws,
            workspace_name: ws_name,
            user_id,
            display_name,
        },
    ));
    out.extend(threads.into_iter().map(
        |(label, agent_id, agent_name, ws, ws_name, user_id, display_name)| ChannelTie {
            kind: "thread_link",
            label,
            agent_id,
            agent_name,
            workspace_id: ws,
            workspace_name: ws_name,
            user_id,
            display_name,
        },
    ));
    Ok(out)
}

/// Live `auo_` org keys whose creator — an effective org owner/admin — has no own access to at
/// least one *shared* workspace of the org (item 4; personal workspaces are the person's alone
/// under D-480/C-1161 and cannot be fixed by adding the creator, so they are not listed).
async fn org_keys(conn: &mut sqlx::PgConnection, org: Uuid) -> Result<Vec<OrgKey>, ApiError> {
    let rows: Vec<OrgKeyRow> =
        sqlx::query_as(
            "SELECT k.id, k.display_prefix, k.user_id, u.display_name, k.last_used_at \
             FROM api_keys k \
             JOIN users u ON u.id = k.user_id \
             WHERE k.workspace_id IS NULL AND k.org_id = $1 AND k.revoked_at IS NULL \
               AND EXISTS (SELECT 1 FROM org_members m \
                           WHERE m.org_id = k.org_id AND m.user_id = k.user_id \
                             AND m.status = 'active' AND m.role IN ('owner','admin')) \
               AND EXISTS (SELECT 1 FROM workspaces w \
                           WHERE w.org_id = k.org_id AND w.personal_owner_user_id IS NULL \
                             AND NOT EXISTS (SELECT 1 FROM own_workspace_access o \
                                             WHERE o.workspace_id = w.id AND o.user_id = k.user_id)) \
             ORDER BY k.display_prefix",
        )
        .bind(org)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    Ok(rows
        .into_iter()
        .map(
            |(key_id, prefix, creator_id, creator_name, last_used_at)| OrgKey {
                key_id,
                prefix,
                creator_id,
                creator_name,
                last_used_at,
            },
        )
        .collect())
}

/// Open approvals in workspaces with no own-right decider (item 5).
async fn undecidable(
    conn: &mut sqlx::PgConnection,
    org: Uuid,
) -> Result<Vec<UndecidableApproval>, ApiError> {
    let rows: Vec<(
        Uuid,
        Uuid,
        String,
        Uuid,
        String,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        "SELECT ap.id, ap.agent_id, coalesce(a.name, '(agent deleted)'), ap.workspace_id, \
             w.name, ap.deadline \
             FROM approvals ap \
             JOIN workspaces w ON w.id = ap.workspace_id \
             LEFT JOIN agents a ON a.id = ap.agent_id \
             WHERE ap.org_id = $1 AND ap.status = 'pending' \
               AND NOT EXISTS (SELECT 1 FROM own_workspace_access o \
                               WHERE o.workspace_id = ap.workspace_id \
                                 AND o.role IN ('approver','owner')) \
             ORDER BY ap.deadline",
    )
    .bind(org)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    Ok(rows
        .into_iter()
        .map(
            |(approval_id, agent_id, agent_name, workspace_id, workspace_name, deadline)| {
                UndecidableApproval {
                    approval_id,
                    agent_id,
                    agent_name,
                    workspace_id,
                    workspace_name,
                    deadline,
                }
            },
        )
        .collect())
}

/// The report over every organization (read-only; nothing is written).
pub async fn run(state: &AppState) -> Result<Report, ApiError> {
    // (FIX-234 review 7) The whole report runs in one `READ ONLY` transaction: a write added
    // inside any helper makes PostgreSQL reject the statement instead of silently committing.
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    run_in(&mut tx).await
}

/// The report over an already-open transaction. Callers must have put it in `READ ONLY` (as
/// [`run`] does); the helpers are plain SELECTs and write nothing.
pub async fn run_in(conn: &mut sqlx::PgConnection) -> Result<Report, ApiError> {
    let orgs: Vec<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, slug, name FROM organizations ORDER BY slug")
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let mut out = Vec::new();
    for (org_id, org_slug, org_name) in orgs {
        out.push(OrgReport {
            org_id,
            org_slug,
            org_name,
            ownerless: ownerless(conn, org_id).await?,
            failing_automations: failing_automations(conn, org_id).await?,
            channel_ties: channel_ties(conn, org_id).await?,
            org_keys: org_keys(conn, org_id).await?,
            undecidable: undecidable(conn, org_id).await?,
        });
    }
    Ok(Report { orgs: out })
}

/// The `--text` rendering: one line per item, grouped by organization.
pub fn text(r: &Report) -> String {
    let mut s = String::new();
    for o in &r.orgs {
        let empty = o.ownerless.is_empty()
            && o.failing_automations.is_empty()
            && o.channel_ties.is_empty()
            && o.org_keys.is_empty()
            && o.undecidable.is_empty();
        if empty {
            continue;
        }
        s.push_str(&format!("organization {} ({})\n", o.org_name, o.org_slug));
        if !o.ownerless.is_empty() {
            s.push_str("  workspaces with no active own-right owner:\n");
            for w in &o.ownerless {
                match &w.personal {
                    Some(p) => s.push_str(&format!(
                        "    {} ({}) PERSONAL — person {} ({}), role there: {}\n",
                        w.name,
                        w.slug,
                        p.display_name,
                        p.user_id,
                        p.role.as_deref().unwrap_or("none"),
                    )),
                    None => s.push_str(&format!("    {} ({})\n", w.name, w.slug)),
                }
            }
        }
        if !o.failing_automations.is_empty() {
            s.push_str("  schedules/webhooks whose fire-time authority will fail:\n");
            for a in &o.failing_automations {
                s.push_str(&format!(
                    "    {} {} ({}) — agent {} ({}) in {} ({}), anchor {}\n",
                    a.kind,
                    a.name,
                    a.id,
                    a.agent_name,
                    a.agent_id,
                    a.workspace_name,
                    a.workspace_id,
                    a.anchor,
                ));
            }
        }
        if !o.channel_ties.is_empty() {
            s.push_str("  channel objects tied to a person without own access:\n");
            for t in &o.channel_ties {
                s.push_str(&format!(
                    "    {} {} — agent {} ({}), person {} ({}), workspace {} ({})\n",
                    t.kind,
                    t.label,
                    t.agent_name,
                    t.agent_id,
                    t.display_name,
                    t.user_id,
                    t.workspace_name,
                    t.workspace_id,
                ));
            }
        }
        if !o.org_keys.is_empty() {
            s.push_str("  live auo_ keys whose creator will lose reach:\n");
            for k in &o.org_keys {
                s.push_str(&format!(
                    "    {} (key {}) — creator {} ({}), last used {}\n",
                    k.prefix,
                    k.key_id,
                    k.creator_name,
                    k.creator_id,
                    k.last_used_at
                        .map(|t| t.to_rfc3339())
                        .unwrap_or_else(|| "never".into()),
                ));
            }
        }
        if !o.undecidable.is_empty() {
            s.push_str("  open approvals with no own-right decider:\n");
            for a in &o.undecidable {
                s.push_str(&format!(
                    "    approval {} — agent {} ({}), workspace {} ({}), deadline {}\n",
                    a.approval_id,
                    a.agent_name,
                    a.agent_id,
                    a.workspace_name,
                    a.workspace_id,
                    a.deadline.to_rfc3339(),
                ));
            }
        }
    }
    if s.is_empty() {
        s.push_str("nothing to fix: every organization keeps its access after the upgrade.\n");
    }
    s
}

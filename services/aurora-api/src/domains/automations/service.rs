//! Reassignment and anchoring. Every function runs inside the caller's transaction and writes
//! the security-critical `automation.reassign` audit row in that same transaction.

use super::dto::{Offboarding, OffboardingItem};
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::schedules::cron;
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::authz;
use aurora_db::repo::automations as repo;
use aurora_db::repo::schedules::{self as sched, ScheduleRow};
use aurora_db::repo::webhooks::{self as hooks, WebhookRow};
use chrono::Utc;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

/// Where an item's authority goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum To {
    User(Uuid),
    Workspace,
}

impl To {
    /// `"workspace"` or a user id.
    pub fn parse(path: &str, v: &str) -> Result<Self, ApiError> {
        if v == "workspace" {
            return Ok(Self::Workspace);
        }
        v.parse()
            .map(Self::User)
            .map_err(|_| bad(path, "a user id or \"workspace\""))
    }
}

/// Who acts, for the audit rows.
pub struct Acting<'a> {
    pub actor: &'a AuditActor,
    pub org: Uuid,
    pub workspace: Uuid,
}

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, msg)])
}

fn unauthorized_target() -> ApiError {
    ApiError::unprocessable(
        "new_creator_unauthorized",
        "The new creator must be an active member who can use the workspace.",
    )
}

/// `creator` / `workspace`, or 400.
pub fn parse_anchor(v: &str) -> Result<&'static str, ApiError> {
    match v {
        "creator" => Ok("creator"),
        "workspace" => Ok("workspace"),
        _ => Err(bad("/anchor", "creator or workspace")),
    }
}

/// The target must be an active org member who can use the workspace.
pub async fn ensure_target(conn: &mut PgConnection, to: To, ws: Uuid) -> Result<(), ApiError> {
    if let To::User(u) = to
        && authz::workspace_access(&mut *conn, u, ws).await?.is_none()
    {
        return Err(unauthorized_target());
    }
    Ok(())
}

fn resolve(to: To, old_creator: Option<Uuid>) -> (Option<Uuid>, &'static str) {
    match to {
        To::User(u) => (Some(u), "creator"),
        To::Workspace => (old_creator, "workspace"),
    }
}

/// The audited item: kind, id and its agent.
struct ItemRef(&'static str, Uuid, Uuid);

async fn audit_row(
    conn: &mut PgConnection,
    a: &Acting<'_>,
    item: ItemRef,
    old: (Option<Uuid>, &str),
    new: (Option<Uuid>, &str),
    to: To,
) -> Result<(), ApiError> {
    let ItemRef(kind, id, agent) = item;
    let ev = AuditEvent::new(a.actor.clone(), "automation.reassign", Outcome::Success)
        .org(a.org)
        .workspace(a.workspace)
        .target(kind, id)
        .metadata(json!({
            "kind": kind, "id": id, "agent_id": agent,
            "to": match to { To::Workspace => json!("workspace"), To::User(u) => json!(u) },
            "old_creator": old.0, "new_creator": new.0,
            "old_anchor": old.1, "new_anchor": new.1,
        }));
    audit::insert_tx(conn, &ev).await.map_err(|e| {
        tracing::error!(error = %e, "automation.reassign audit write failed");
        ApiError::unavailable("audit_unavailable", "The audit log is unavailable.")
    })?;
    Ok(())
}

pub async fn reassign_schedule(
    conn: &mut PgConnection,
    a: &Acting<'_>,
    row: &ScheduleRow,
    to: To,
    re_enable: bool,
) -> Result<(), ApiError> {
    ensure_target(conn, to, a.workspace).await?;
    let (creator, anchor) = resolve(to, row.creator_user_id);
    let next =
        if re_enable && !row.enabled && row.disabled_reason.as_deref() == Some("authorization") {
            Some(cron::validate(&row.cron, &row.timezone, Utc::now())?)
        } else {
            None
        };
    sched::set_authority(conn, row.id, creator, anchor, next).await?;
    audit_row(
        conn,
        a,
        ItemRef("schedule", row.id, row.agent_id),
        (row.creator_user_id, &row.anchor),
        (creator, anchor),
        to,
    )
    .await
}

pub async fn reassign_webhook(
    conn: &mut PgConnection,
    a: &Acting<'_>,
    row: &WebhookRow,
    to: To,
    re_enable: bool,
) -> Result<(), ApiError> {
    ensure_target(conn, to, a.workspace).await?;
    let (creator, anchor) = resolve(to, row.creator_user_id);
    let re = re_enable && !row.enabled && row.disabled_reason.as_deref() == Some("authorization");
    hooks::set_authority(conn, row.id, creator, anchor, re).await?;
    audit_row(
        conn,
        a,
        ItemRef("webhook", row.id, row.agent_id),
        (row.creator_user_id, &row.anchor),
        (creator, anchor),
        to,
    )
    .await
}

/// Switch an item's anchor. To `workspace`: the creator stays as provenance. To `creator`: the
/// current creator becomes the authority again and must still be authorized (422 otherwise).
pub fn anchor_target(anchor: &str, creator: Option<Uuid>) -> Result<To, ApiError> {
    if anchor == "workspace" {
        Ok(To::Workspace)
    } else {
        creator.map(To::User).ok_or_else(unauthorized_target)
    }
}

/// Reassign every item of `from` in the workspace (all, or only `ids`). Without `ids` only
/// creator-anchored items move: workspace-anchored ones do not depend on the person.
/// Returns the number reassigned.
pub async fn reassign_all(
    conn: &mut PgConnection,
    a: &Acting<'_>,
    from: Uuid,
    to: To,
    schedules: bool,
    webhooks: bool,
    ids: Option<&[Uuid]>,
) -> Result<i64, ApiError> {
    let mut n = 0;
    let mut sr = Vec::new();
    let mut wr = Vec::new();
    if schedules {
        sr = sched::lock_by_creator(conn, a.workspace, from, ids).await?;
    }
    if webhooks {
        wr = hooks::lock_by_creator(conn, a.workspace, from, ids).await?;
    }
    if ids.is_none() {
        sr.retain(|r| r.anchor == "creator");
        wr.retain(|r| r.anchor == "creator");
    }
    if sr.is_empty() && wr.is_empty() {
        return Ok(0);
    }
    if to == To::User(from) {
        return Err(bad("/to", "must differ from the current creator"));
    }
    for r in &sr {
        reassign_schedule(conn, a, r, to, false).await?;
        n += 1;
    }
    for r in &wr {
        reassign_webhook(conn, a, r, to, false).await?;
        n += 1;
    }
    Ok(n)
}

/// `reassign_to` of a member removal/suspension: every workspace of the org, same transaction.
pub async fn reassign_org_member(
    conn: &mut PgConnection,
    actor: &AuditActor,
    org: Uuid,
    from: Uuid,
    to: To,
) -> Result<i64, ApiError> {
    if to == To::User(from) {
        return Err(bad(
            "/reassign_to",
            "must differ from the person being removed",
        ));
    }
    // A typo must not be silently accepted when the person happens to have no automations.
    if let To::User(u) = to
        && !authz::org_membership(&mut *conn, u, org)
            .await?
            .is_some_and(|m| m.is_active())
    {
        return Err(unauthorized_target());
    }
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM workspaces WHERE org_id = $1 ORDER BY id")
            .bind(org)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let mut n = 0;
    for ws in ids {
        let a = Acting {
            actor,
            org,
            workspace: ws,
        };
        n += reassign_all(conn, &a, from, to, true, true, None).await?;
    }
    Ok(n)
}

/// What removing `user` would disable, for the org (`workspace = None`) or one workspace.
pub async fn offboarding(
    conn: &mut PgConnection,
    org: Uuid,
    workspace: Option<Uuid>,
    user: Uuid,
) -> Result<Offboarding, ApiError> {
    let items = |v: Vec<repo::OffboardRow>| -> Vec<OffboardingItem> {
        v.into_iter().map(OffboardingItem::from).collect()
    };
    let mut out = Offboarding {
        schedules: items(repo::schedules(&mut *conn, org, workspace, user).await?),
        webhooks: items(repo::webhooks(&mut *conn, org, workspace, user).await?),
        peer_tasks: items(repo::peer_tasks(&mut *conn, org, workspace, user).await?),
        api_keys: repo::api_keys(&mut *conn, org, workspace, user).await?,
    };
    // (P4-GRP-1) "What would break" for people whose access comes through groups: name the
    // groups that currently give the person access to each item's workspace.
    let ws_ids: Vec<Uuid> = out
        .schedules
        .iter()
        .chain(&out.webhooks)
        .chain(&out.peer_tasks)
        .map(|i| i.workspace.id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let via =
        aurora_db::repo::groups::via_groups(&mut *conn, org, user, &ws_ids).await?;
    for item in out
        .schedules
        .iter_mut()
        .chain(out.webhooks.iter_mut())
        .chain(out.peer_tasks.iter_mut())
    {
        let mut seen = std::collections::BTreeSet::new();
        item.via_groups = via
            .iter()
            .filter(|(w, g, _)| *w == item.workspace.id && seen.insert(*g))
            .map(|(_, g, n)| super::dto::NamedRef {
                id: *g,
                name: n.clone(),
            })
            .collect();
    }
    Ok(out)
}

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use std::net::IpAddr;
use uuid::Uuid;

pub struct NewAudit<'a> {
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub actor_kind: &'a str,
    pub actor_id: Option<Uuid>,
    pub actor_label: &'a str,
    pub action: &'a str,
    pub target_kind: Option<&'a str>,
    pub target_id: Option<&'a str>,
    pub outcome: &'a str,
    pub source: &'a str,
    pub ip: Option<IpAddr>,
    pub metadata: &'a serde_json::Value,
}

pub async fn insert<'e, E: PgExecutor<'e>>(exec: E, a: &NewAudit<'_>) -> Result<Uuid, DbError> {
    let id = aurora_core::ids::new_id();
    // RFE-008: the flag is derived from the action, so it can never disagree with the set.
    let critical = a.outcome == "success" && aurora_core::audit::is_critical(a.action);
    sqlx::query(
        "INSERT INTO audit_log (id, org_id, workspace_id, actor_kind, actor_id, actor_label,
            action, target_kind, target_id, outcome, source, ip, metadata, critical)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::text::inet,$13,$14)",
    )
    .bind(id)
    .bind(a.org_id)
    .bind(a.workspace_id)
    .bind(a.actor_kind)
    .bind(a.actor_id)
    .bind(a.actor_label)
    .bind(a.action)
    .bind(a.target_kind)
    .bind(a.target_id)
    .bind(a.outcome)
    .bind(a.source)
    .bind(a.ip.map(|i| i.to_string()))
    .bind(a.metadata)
    .bind(critical)
    .execute(exec)
    .await?;
    Ok(id)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuditRow {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub actor_kind: String,
    pub actor_id: Option<Uuid>,
    pub actor_label: String,
    pub action: String,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub outcome: String,
    pub source: String,
    pub ip: Option<String>,
    pub metadata: serde_json::Value,
    pub critical: bool,
    pub created_at: DateTime<Utc>,
}

/// An audit row with the organization's slug (instance audit view, FIX-125).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InstanceAuditRow {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub org_slug: Option<String>,
    pub workspace_id: Option<Uuid>,
    pub actor_kind: String,
    pub actor_id: Option<Uuid>,
    pub actor_label: String,
    pub action: String,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub outcome: String,
    pub source: String,
    pub ip: Option<String>,
    pub metadata: serde_json::Value,
    pub critical: bool,
    pub created_at: DateTime<Utc>,
}

/// Filters of the instance audit list.
#[derive(Debug, Default, Clone)]
pub struct InstanceFilter<'a> {
    /// `false`: only `org_id IS NULL` rows; `true`: every row.
    pub all_orgs: bool,
    pub org: Option<Uuid>,
    /// Action prefix (literal, not a pattern).
    pub action_prefix: Option<&'a str>,
    pub actor_id: Option<Uuid>,
    pub critical_only: bool,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

/// Instance audit list, newest first, keyset on `(created_at, id)`.
pub async fn list_instance<'e, E: PgExecutor<'e>>(
    exec: E,
    f: &InstanceFilter<'_>,
    after: After,
    fetch: i64,
) -> Result<Vec<InstanceAuditRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(
        "SELECT a.id, a.org_id, o.slug AS org_slug, a.workspace_id, a.actor_kind, a.actor_id,
                a.actor_label, a.action, a.target_kind, a.target_id, a.outcome, a.source,
                host(a.ip) AS ip, a.metadata, a.critical, a.created_at
         FROM audit_log a LEFT JOIN organizations o ON o.id = a.org_id
         WHERE ($1 OR $2::uuid IS NOT NULL OR a.org_id IS NULL)
           AND ($2::uuid IS NULL OR a.org_id = $2)
           AND ($3::text IS NULL OR starts_with(a.action, $3))
           AND ($4::uuid IS NULL OR a.actor_id = $4)
           AND (NOT $5 OR a.critical)
           AND ($6::timestamptz IS NULL OR a.created_at >= $6)
           AND ($7::timestamptz IS NULL OR a.created_at < $7)
           AND ($8::timestamptz IS NULL OR (a.created_at, a.id) < ($8, $9))
         ORDER BY a.created_at DESC, a.id DESC LIMIT $10",
    )
    .bind(f.all_orgs)
    .bind(f.org)
    .bind(f.action_prefix)
    .bind(f.actor_id)
    .bind(f.critical_only)
    .bind(f.since)
    .bind(f.until)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Which scope to list.
#[derive(Debug, Clone, Copy)]
pub enum Scope {
    Org(Uuid),
    Workspace(Uuid),
}

/// Newest first, keyset on `(created_at, id)`.
pub async fn list<'e, E: PgExecutor<'e>>(
    exec: E,
    scope: Scope,
    action: Option<&str>,
    actor_id: Option<Uuid>,
    critical_only: bool,
    after: After,
    fetch: i64,
) -> Result<Vec<AuditRow>, DbError> {
    let (t, id) = split(after);
    let (org, ws) = match scope {
        Scope::Org(o) => (Some(o), None),
        Scope::Workspace(w) => (None, Some(w)),
    };
    Ok(sqlx::query_as(
        "SELECT id, org_id, workspace_id, actor_kind, actor_id, actor_label, action, target_kind,
                target_id, outcome, source, host(ip) AS ip, metadata, critical, created_at
         FROM audit_log
         WHERE ($1::uuid IS NULL OR org_id = $1) AND ($2::uuid IS NULL OR workspace_id = $2)
           AND ($3::text IS NULL OR action = $3) AND ($4::uuid IS NULL OR actor_id = $4)
           AND (NOT $8 OR critical)
           AND ($5::timestamptz IS NULL OR (created_at, id) < ($5, $6))
         ORDER BY created_at DESC, id DESC LIMIT $7",
    )
    .bind(org)
    .bind(ws)
    .bind(action)
    .bind(actor_id)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .bind(critical_only)
    .fetch_all(exec)
    .await?)
}

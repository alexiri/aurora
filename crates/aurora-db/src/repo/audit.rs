use crate::DbError;
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
    sqlx::query(
        "INSERT INTO audit_log (id, org_id, workspace_id, actor_kind, actor_id, actor_label,
            action, target_kind, target_id, outcome, source, ip, metadata)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::text::inet,$13)",
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
    .execute(exec)
    .await?;
    Ok(id)
}

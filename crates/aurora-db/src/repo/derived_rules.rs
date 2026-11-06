//! Derived (compiled) policy rules (docs/03 `derived_rules`).

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

/// Upsert one rule of a source; `logical_id` is kept stable on update.
pub async fn upsert<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    workspace_id: Option<Uuid>,
    source_kind: &str,
    source_id: Uuid,
    ordinal: i32,
    rule: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO derived_rules (id, org_id, workspace_id, source_kind, source_id, ordinal, logical_id, rule) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
         ON CONFLICT (source_kind, source_id, ordinal) DO UPDATE SET rule = EXCLUDED.rule, \
         org_id = EXCLUDED.org_id, workspace_id = EXCLUDED.workspace_id, updated_at = now()",
    )
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(workspace_id)
    .bind(source_kind)
    .bind(source_id)
    .bind(ordinal)
    .bind(Uuid::now_v7())
    .bind(rule)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn delete_for_source<'e, E: PgExecutor<'e>>(
    exec: E,
    source_kind: &str,
    source_id: Uuid,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM derived_rules WHERE source_kind = $1 AND source_id = $2")
        .bind(source_kind)
        .bind(source_id)
        .execute(exec)
        .await?;
    Ok(())
}

/// Remove the compiled rules of every grant of a secret (call before deleting the secret,
/// whose grants cascade without touching derived rules).
pub async fn delete_for_secret_grants<'e, E: PgExecutor<'e>>(
    exec: E,
    secret_id: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "DELETE FROM derived_rules WHERE source_kind = 'grant' \
         AND source_id IN (SELECT id FROM grants WHERE secret_id = $1)",
    )
    .bind(secret_id)
    .execute(exec)
    .await?;
    Ok(())
}

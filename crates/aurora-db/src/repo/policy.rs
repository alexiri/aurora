//! Policy generations (only what tenancy needs).

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

/// Insert an empty generation (`default_action = none`). Published rows get
/// `published_at = now()`; `workspace = None` is the org scope.
pub async fn insert_empty_generation<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    number: i32,
    published: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO policy_generations (id, org_id, workspace_id, number, status, published_at)
         VALUES ($1,$2,$3,$4,$5, CASE WHEN $6 THEN now() END)",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(workspace)
    .bind(number)
    .bind(if published { "published" } else { "draft" })
    .bind(published)
    .execute(exec)
    .await?;
    Ok(())
}

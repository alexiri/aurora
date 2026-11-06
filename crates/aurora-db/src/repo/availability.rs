//! App availability (`organizations.app_posture` + `org_app_availability`, 07 §App availability).

use crate::DbError;
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

/// `restricted` or `open`; `None` when the org does not exist.
pub async fn posture<'e, E: PgExecutor<'e>>(exec: E, org: Uuid) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT app_posture FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_optional(exec)
            .await?,
    )
}

/// Allow-list rows `(app_id, workspace_id)`; a NULL workspace allows the whole org.
pub async fn list<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Vec<(String, Option<Uuid>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT app_id, workspace_id FROM org_app_availability WHERE org_id = $1 \
         ORDER BY app_id, workspace_id NULLS FIRST",
    )
    .bind(org)
    .fetch_all(exec)
    .await?)
}

/// Replace the posture and the allow-list atomically (caller owns the transaction).
pub async fn replace(
    conn: &mut PgConnection,
    org: Uuid,
    posture: &str,
    rows: &[(String, Option<Uuid>)],
) -> Result<(), DbError> {
    sqlx::query("UPDATE organizations SET app_posture = $2, updated_at = now() WHERE id = $1")
        .bind(org)
        .bind(posture)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM org_app_availability WHERE org_id = $1")
        .bind(org)
        .execute(&mut *conn)
        .await?;
    for (app, ws) in rows {
        sqlx::query(
            "INSERT INTO org_app_availability (org_id, app_id, workspace_id) VALUES ($1,$2,$3) \
             ON CONFLICT DO NOTHING",
        )
        .bind(org)
        .bind(app)
        .bind(ws)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Workspaces of the org among `ids` (validation of allow-list rows).
pub async fn workspaces_in_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM workspaces WHERE org_id = $1 AND id = ANY($2)")
            .bind(org)
            .bind(ids)
            .fetch_all(exec)
            .await?,
    )
}

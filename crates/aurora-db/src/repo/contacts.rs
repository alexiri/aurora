//! Who to ask (P5-UX-2, C-981): display names of the people who hold a role, for a refusal or
//! an empty state. Names only: never an e-mail, never an instance admin (the instance admin is
//! named by no query here, and an instance admin who is also a member is named only as the
//! member they are). Disabled users are skipped. Owners first, then by name.

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

/// The most names any answer carries.
pub const CONTACTS_MAX: i64 = 3;

/// Organization owners (`owners_only`) or owners and admins, by **effective** role (a group can
/// grant the role, D-130).
pub async fn org_contacts<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    owners_only: bool,
) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT u.display_name
           FROM effective_org_members e
           JOIN users u ON u.id = e.user_id AND u.disabled_at IS NULL
          WHERE e.org_id = $1 AND (e.role = 'owner' OR (NOT $2 AND e.role = 'admin'))
          ORDER BY (e.role = 'owner') DESC, u.display_name, u.id
          LIMIT $3",
    )
    .bind(org)
    .bind(owners_only)
    .bind(CONTACTS_MAX)
    .fetch_all(exec)
    .await?)
}

/// People whose **effective** workspace role is `owner` (in their own right: D-477, an org role
/// confers no workspace right), or `owner` and `approver` with `or_approver`. Owners come first.
pub async fn workspace_contacts<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Uuid,
    or_approver: bool,
) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT u.display_name
           FROM effective_workspace_access ew
           JOIN users u ON u.id = ew.user_id AND u.disabled_at IS NULL
          WHERE ew.workspace_id = $1 AND (ew.role = 'owner' OR ($2 AND ew.role = 'approver'))
          ORDER BY (ew.role = 'owner') DESC, u.display_name, u.id
          LIMIT $3",
    )
    .bind(workspace)
    .bind(or_approver)
    .bind(CONTACTS_MAX)
    .fetch_all(exec)
    .await?)
}

/// The slugs of an organization and (optionally) one of its workspaces, for dashboard paths.
pub async fn slugs<'e, E: PgExecutor<'e> + Copy>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
) -> Result<Option<(String, Option<String>)>, DbError> {
    let Some(o): Option<String> =
        sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_optional(exec)
            .await?
    else {
        return Ok(None);
    };
    let w = match workspace {
        Some(w) => {
            sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1 AND org_id = $2")
                .bind(w)
                .bind(org)
                .fetch_optional(exec)
                .await?
        }
        None => None,
    };
    Ok(Some((o, w)))
}

//! The `organizations.settings.scim` jsonb (docs/03 §Planned changes): read and partial
//! update of the org's SCIM settings. Defaults = SCIM off, no domains, not exclusive.

use super::auth::OrgScim;
use aurora_db::DbError;
use sqlx::PgConnection;
use uuid::Uuid;

/// The org's settings (`organizations.settings`); `{}` when absent.
pub async fn org_settings(
    exec: impl sqlx::PgExecutor<'_>,
    org: Uuid,
) -> Result<serde_json::Value, DbError> {
    Ok(
        sqlx::query_scalar("SELECT settings FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_one(exec)
            .await?,
    )
}

/// The org's SCIM settings as [`OrgScim`].
pub async fn org_scim(exec: impl sqlx::PgExecutor<'_>, org: Uuid) -> Result<OrgScim, DbError> {
    let v = org_settings(exec, org).await?;
    Ok(OrgScim::from_settings(
        org,
        v.get("scim").unwrap_or(&serde_json::Value::Null),
    ))
}

/// Merge `patch` into `organizations.settings.scim` (a JSON `object`) and write it back.
/// `None` fields keep the stored value; `email_domains: Some(vec![])` clears the list.
pub async fn update_scim_settings(
    conn: &mut PgConnection,
    org: Uuid,
    enabled: Option<bool>,
    email_domains: Option<Vec<String>>,
    exclusive: Option<bool>,
) -> Result<serde_json::Value, DbError> {
    let settings = org_settings(&mut *conn, org).await?;
    let mut scim = settings
        .get("scim")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    if let Some(e) = enabled {
        scim.insert("enabled".to_owned(), serde_json::Value::Bool(e));
    }
    if let Some(d) = email_domains {
        scim.insert(
            "email_domains".to_owned(),
            serde_json::Value::Array(d.into_iter().map(serde_json::Value::String).collect()),
        );
    }
    if let Some(x) = exclusive {
        scim.insert("exclusive".to_owned(), serde_json::Value::Bool(x));
    }
    let mut next = settings.as_object().cloned().unwrap_or_default();
    next.insert("scim".to_owned(), serde_json::Value::Object(scim));
    let next = serde_json::Value::Object(next);
    sqlx::query("UPDATE organizations SET settings = $2, updated_at = now() WHERE id = $1")
        .bind(org)
        .bind(&next)
        .execute(&mut *conn)
        .await?;
    Ok(next.get("scim").cloned().unwrap_or_default())
}

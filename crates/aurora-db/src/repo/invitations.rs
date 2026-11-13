//! Organization invitations (BE-013). Tokens are stored only as hashes and never selected.

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

/// An invitation as listed; `status` is the effective status (`pending` past
/// `expires_at` reads as `expired`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InvitationRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub role: String,
    pub workspace_ids: Vec<Uuid>,
    pub approver_workspace_ids: Vec<Uuid>,
    pub status: String,
    pub invited_by: Option<Uuid>,
    pub invited_by_name: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

const SEL: &str = "i.id, i.org_id, i.email::text AS email, i.role, i.workspace_ids,
    i.approver_workspace_ids,
    CASE WHEN i.status = 'pending' AND i.expires_at <= now() THEN 'expired' ELSE i.status END AS status,
    i.invited_by, u.display_name AS invited_by_name, i.expires_at, i.created_at";
const FROM: &str = "invitations i LEFT JOIN users u ON u.id = i.invited_by";

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<InvitationRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {SEL} FROM {FROM} WHERE i.id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn list<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    status: Option<&str>,
    after: After,
    fetch: i64,
) -> Result<Vec<InvitationRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT * FROM (SELECT {SEL} FROM {FROM} WHERE i.org_id = $1) x
         WHERE ($2::text IS NULL OR x.status = $2)
         AND ($3::timestamptz IS NULL OR (x.created_at, x.id) < ($3, $4))
         ORDER BY x.created_at DESC, x.id DESC LIMIT $5"
    ))
    .bind(org)
    .bind(status)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Lock the org row (serializes seat accounting); returns `(name, settings)`.
pub async fn lock_org(
    conn: &mut PgConnection,
    org: Uuid,
) -> Result<Option<(String, serde_json::Value)>, DbError> {
    Ok(
        sqlx::query_as("SELECT name, settings FROM organizations WHERE id = $1 FOR UPDATE")
            .bind(org)
            .fetch_optional(conn)
            .await?,
    )
}

/// Does a user with this email have any membership row (active or suspended) in the org?
pub async fn is_member<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    email: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM org_members m JOIN users u ON u.id = m.user_id
         WHERE m.org_id = $1 AND u.email = $2::citext)",
    )
    .bind(org)
    .bind(email)
    .fetch_one(exec)
    .await?)
}

/// Seats in use against `max_members`: `active` members and (D-175, FIX-082) `pending` ones
/// SCIM prepared for an existing account; a suspended member holds none.
pub async fn count_seats<'e, E: PgExecutor<'e>>(exec: E, org: Uuid) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM org_members WHERE org_id = $1 AND status IN ('active', 'pending')",
    )
    .bind(org)
    .fetch_one(exec)
    .await?)
}

/// Live (unexpired) pending invitations, excluding the one for `except_email`.
pub async fn count_pending_except<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    except_email: &str,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM invitations WHERE org_id = $1 AND status = 'pending'
         AND expires_at > now() AND email <> $2::citext",
    )
    .bind(org)
    .bind(except_email)
    .fetch_one(exec)
    .await?)
}

/// The subset of `ids` that are workspaces of `org`.
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

pub struct NewInvitation<'a> {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: &'a str,
    pub role: &'a str,
    pub workspace_ids: &'a [Uuid],
    pub approver_workspace_ids: &'a [Uuid],
    pub token_hash: &'a [u8; 32],
    pub invited_by: Uuid,
}

/// Insert, or re-invite the pending invitation for `(org, email)`: rotates the token and
/// resets expiry. Returns the invitation id.
pub async fn upsert_pending<'e, E: PgExecutor<'e>>(
    exec: E,
    n: &NewInvitation<'_>,
) -> Result<Uuid, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO invitations (id, org_id, email, role, workspace_ids, approver_workspace_ids, token_hash, status, invited_by)
         VALUES ($1, $2, $3::citext, $4, $5, $8, $6, 'pending', $7)
         ON CONFLICT (org_id, email) WHERE status = 'pending' DO UPDATE SET
           role = EXCLUDED.role, workspace_ids = EXCLUDED.workspace_ids,
           approver_workspace_ids = EXCLUDED.approver_workspace_ids,
           token_hash = EXCLUDED.token_hash, invited_by = EXCLUDED.invited_by,
           expires_at = now() + interval '7 days', updated_at = now()
         RETURNING id",
    )
    .bind(n.id)
    .bind(n.org_id)
    .bind(n.email)
    .bind(n.role)
    .bind(n.workspace_ids)
    .bind(&n.token_hash[..])
    .bind(n.invited_by)
    .bind(n.approver_workspace_ids)
    .fetch_one(exec)
    .await?)
}

/// Cancel a pending invitation; returns whether one changed.
pub async fn cancel<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE invitations SET status = 'cancelled', updated_at = now()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PreviewRow {
    pub org_id: Uuid,
    pub org_slug: String,
    pub org_name: String,
    pub email: String,
    pub role: String,
    pub inviter_name: Option<String>,
    pub expires_at: DateTime<Utc>,
}

/// A pending, unexpired invitation by token hash.
pub async fn preview<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8; 32],
) -> Result<Option<PreviewRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT i.org_id, o.slug AS org_slug, o.name AS org_name, i.email::text AS email, i.role,
                u.display_name AS inviter_name, i.expires_at
         FROM invitations i JOIN organizations o ON o.id = i.org_id
         LEFT JOIN users u ON u.id = i.invited_by
         WHERE i.token_hash = $1 AND i.status = 'pending' AND i.expires_at > now()",
    )
    .bind(&token_hash[..])
    .fetch_optional(exec)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AcceptRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub role: String,
    pub workspace_ids: Vec<Uuid>,
    pub approver_workspace_ids: Vec<Uuid>,
    pub expired: bool,
}

/// Lock a pending invitation by token hash (expired ones are flagged, not hidden).
pub async fn lock_pending_by_token(
    conn: &mut PgConnection,
    token_hash: &[u8; 32],
) -> Result<Option<AcceptRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, org_id, email::text AS email, role, workspace_ids, approver_workspace_ids,
                expires_at <= now() AS expired
         FROM invitations WHERE token_hash = $1 AND status = 'pending' FOR UPDATE",
    )
    .bind(&token_hash[..])
    .fetch_optional(conn)
    .await?)
}

pub async fn mark_accepted<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    user: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE invitations SET status = 'accepted', accepted_by = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(user)
    .execute(exec)
    .await?;
    Ok(())
}

/// Pending, unexpired invitations addressed to `email` (case-insensitive); rows locked. Used at
/// the first OIDC sign-in (docs/04 §Sign-up semantics under OIDC-only).
pub async fn lock_pending_by_email(
    conn: &mut PgConnection,
    email: &str,
) -> Result<Vec<AcceptRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, org_id, email::text AS email, role, workspace_ids, approver_workspace_ids,
                false AS expired
         FROM invitations
         WHERE email = $1::citext AND status = 'pending' AND expires_at > now()
         ORDER BY created_at FOR UPDATE",
    )
    .bind(email)
    .fetch_all(conn)
    .await?)
}

/// Is there a pending, unexpired invitation addressed to `email`?
pub async fn has_pending_for_email<'e, E: PgExecutor<'e>>(
    exec: E,
    email: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM invitations
         WHERE email = $1::citext AND status = 'pending' AND expires_at > now())",
    )
    .bind(email)
    .fetch_one(exec)
    .await?)
}

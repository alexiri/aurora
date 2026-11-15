//! SCIM 2.0 provisioning (phase 4, P4-GRP-2, D-132, D-135, SEC-72; docs/03 §scim_tokens,
//! §scim_users): the token store and the per-org link table.
//!
//! This module only stores and reads. All semantics (domain allow-lists, owners never mutated,
//! deprovisioning effects) live in the API's SCIM service, which runs every change in one
//! transaction with its critical audit rows. Token liveness is read on every SCIM request;
//! there is no cache.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

// ---- tokens ----------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TokenRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub token_hash: Vec<u8>,
    pub display_prefix: String,
    pub created_by: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const TOKEN_COLS: &str = "id, org_id, name, token_hash, display_prefix, created_by, expires_at, \
    revoked_at, last_used_at, created_at";

impl TokenRow {
    /// Dead when revoked or past `expires_at` (read-time check; there is no non-expiring token).
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at > now
    }
}

/// The token with exactly this hash (the lookup is on the unique index; the caller checks
/// liveness, so a dead token never looks like an unknown one to the audit trail).
pub async fn token_by_hash<'e, E: PgExecutor<'e>>(
    exec: E,
    hash: &[u8],
) -> Result<Option<TokenRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {TOKEN_COLS} FROM scim_tokens WHERE token_hash = $1"
    ))
    .bind(hash)
    .fetch_optional(exec)
    .await?)
}

pub async fn token_by_id<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    id: Uuid,
) -> Result<Option<TokenRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {TOKEN_COLS} FROM scim_tokens WHERE org_id = $1 AND id = $2"
    ))
    .bind(org)
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// The org's tokens, newest first (management list; the hash column is projected away by the
/// API layer).
pub async fn tokens_of<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Vec<TokenRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {TOKEN_COLS} FROM scim_tokens WHERE org_id = $1 ORDER BY created_at DESC, id DESC"
    ))
    .bind(org)
    .fetch_all(exec)
    .await?)
}

/// Number of *active* (unrevoked, unexpired) tokens of the org: the `SCIM_TOKENS_PER_ORG` cap.
pub async fn live_token_count<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    now: DateTime<Utc>,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM scim_tokens
         WHERE org_id = $1 AND revoked_at IS NULL AND expires_at > $2",
    )
    .bind(org)
    .bind(now)
    .fetch_one(exec)
    .await?)
}

pub struct NewToken<'a> {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: &'a str,
    pub token_hash: &'a [u8],
    pub display_prefix: &'a str,
    pub created_by: Uuid,
    pub expires_at: DateTime<Utc>,
}

pub async fn insert_token<'e, E: PgExecutor<'e>>(
    exec: E,
    t: &NewToken<'_>,
) -> Result<TokenRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO scim_tokens (id, org_id, name, token_hash, display_prefix, created_by, \
             expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING {TOKEN_COLS}"
    ))
    .bind(t.id)
    .bind(t.org_id)
    .bind(t.name)
    .bind(t.token_hash)
    .bind(t.display_prefix)
    .bind(t.created_by)
    .bind(t.expires_at)
    .fetch_one(exec)
    .await?)
}

/// Revoke a token of `org` (idempotent: `false` when already revoked or foreign).
pub async fn revoke_token(conn: &mut PgConnection, org: Uuid, id: Uuid) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE scim_tokens SET revoked_at = now(), updated_at = now()
         WHERE org_id = $1 AND id = $2 AND revoked_at IS NULL",
    )
    .bind(org)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Churn column: at most one write per minute per token (docs/03 §scim_tokens).
pub async fn touch_token<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE scim_tokens SET last_used_at = now()
         WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Tokens expiring within `days` of `now` (the expiry-notice job; the notice fires once per
/// token per threshold: `expires_notified_at` is advisory and re-derivable from the audit log).
pub async fn tokens_expiring_within<'e, E: PgExecutor<'e>>(
    exec: E,
    days: i64,
    now: DateTime<Utc>,
) -> Result<Vec<(Uuid, Uuid, String, DateTime<Utc>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.org_id, t.name, t.expires_at FROM scim_tokens t
         WHERE t.revoked_at IS NULL AND t.expires_at > $2
           AND t.expires_at <= $2 + make_interval(days => $1::int)
           AND NOT EXISTS (
             SELECT 1 FROM audit_log a
              WHERE a.target_kind = 'scim_token' AND a.target_id = t.id::text
                AND a.action = 'scim_token.expiry_notice'
                AND a.metadata @> jsonb_build_object('threshold_days', $1))
         ORDER BY t.expires_at",
    )
    .bind(days)
    .bind(now)
    .fetch_all(exec)
    .await?)
}

// ---- users (the link table) ------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub external_id: Option<String>,
    pub user_name: String,
    pub display_name: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub state: String,
    pub suspended_by_scim: bool,
    pub removed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const USER_COLS: &str = "id, org_id, user_id, external_id, user_name::text AS user_name, \
    display_name, given_name, family_name, state, suspended_by_scim, removed_at, created_at, \
    updated_at";

/// The org's link for one Aurora user, any state (the sign-in and deprovisioning paths).
pub async fn user_link<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<Option<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM scim_users WHERE org_id = $1 AND user_id = $2"
    ))
    .bind(org)
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

/// The org's link with one SCIM id (resource handlers).
pub async fn user_by_id<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    id: Uuid,
) -> Result<Option<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM scim_users WHERE org_id = $1 AND id = $2"
    ))
    .bind(org)
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// An unremoved link of the org with one `externalId` (uniqueness on POST).
pub async fn user_by_external_id<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    external_id: &str,
) -> Result<Option<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM scim_users
         WHERE org_id = $1 AND external_id = $2 AND state <> 'removed'"
    ))
    .bind(org)
    .bind(external_id)
    .fetch_optional(exec)
    .await?)
}

/// Number of non-removed links of the org (SCIM list `totalResults`), optionally only those
/// whose `displayName` equals `display_name` case-insensitively. No upper bound: the list
/// pages in SQL ([`users_window`]), never through an in-memory window (FIX-081).
pub async fn users_count<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    display_name: Option<&str>,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM scim_users
         WHERE org_id = $1 AND state <> 'removed'
           AND ($2::text IS NULL OR lower(display_name) = lower($2))",
    )
    .bind(org)
    .bind(display_name)
    .fetch_one(exec)
    .await?)
}

/// One page of the non-removed links of the org (SCIM list), stable `created_at, id` order,
/// `OFFSET`/`LIMIT` in SQL; optionally filtered by `displayName` (case-insensitive).
pub async fn users_window<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    display_name: Option<&str>,
    offset: i64,
    limit: i64,
) -> Result<Vec<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM scim_users
         WHERE org_id = $1 AND state <> 'removed'
           AND ($2::text IS NULL OR lower(display_name) = lower($2))
         ORDER BY created_at, id OFFSET $3 LIMIT $4"
    ))
    .bind(org)
    .bind(display_name)
    .bind(offset)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// Full row set including `removed` tombstones (the Directory sync status counts).
pub async fn user_counts<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<(i64, i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE state = 'active'),
                count(*) FILTER (WHERE state = 'suspended'),
                count(*) FILTER (WHERE state = 'removed')
         FROM scim_users WHERE org_id = $1",
    )
    .bind(org)
    .fetch_one(exec)
    .await?)
}

/// Filter `userName eq "…"` (case-insensitive; the column is `citext`).
pub async fn users_by_user_name<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user_name: &str,
) -> Result<Vec<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM scim_users WHERE org_id = $1 AND user_name = $2
         ORDER BY created_at, id"
    ))
    .bind(org)
    .bind(user_name)
    .fetch_all(exec)
    .await?)
}

/// Filter `externalId eq "…"` (case-sensitive).
pub async fn users_by_external_id<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    external_id: &str,
) -> Result<Vec<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM scim_users WHERE org_id = $1 AND external_id = $2
         ORDER BY created_at, id"
    ))
    .bind(org)
    .bind(external_id)
    .fetch_all(exec)
    .await?)
}

/// Filter `emails.value eq "…"` (case-insensitive) — the stored `user_name` e-mail.
pub async fn users_by_email<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    email: &str,
) -> Result<Vec<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM scim_users WHERE org_id = $1 AND user_name = $2
         ORDER BY created_at, id"
    ))
    .bind(org)
    .bind(email)
    .fetch_all(exec)
    .await?)
}

/// Filter `id eq "…"` (parse checked by the caller).
pub async fn users_by_scim_id<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    id: Uuid,
) -> Result<Vec<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM scim_users WHERE org_id = $1 AND id = $2"
    ))
    .bind(org)
    .bind(id)
    .fetch_all(exec)
    .await?)
}

pub struct NewScimUser<'a> {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub external_id: Option<&'a str>,
    pub user_name: &'a str,
    pub display_name: Option<&'a str>,
    pub given_name: Option<&'a str>,
    pub family_name: Option<&'a str>,
    pub state: &'a str,
    /// True when the (possibly adopted) membership SCIM suspended at provisioning time.
    pub suspended_by_scim: bool,
}

pub async fn insert_user<'e, E: PgExecutor<'e>>(
    exec: E,
    u: &NewScimUser<'_>,
) -> Result<UserRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO scim_users (id, org_id, user_id, external_id, user_name, display_name, \
             given_name, family_name, state, suspended_by_scim) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         RETURNING {USER_COLS}"
    ))
    .bind(u.id)
    .bind(u.org_id)
    .bind(u.user_id)
    .bind(u.external_id)
    .bind(u.user_name)
    .bind(u.display_name)
    .bind(u.given_name)
    .bind(u.family_name)
    .bind(u.state)
    .bind(u.suspended_by_scim)
    .fetch_one(exec)
    .await?)
}

/// The org's link with one SCIM id, row-locked (`FOR UPDATE`) for the `PUT`/`PATCH`
/// transaction: two updates of one link serialize instead of racing on its `user_id`.
pub async fn lock_user(
    conn: &mut PgConnection,
    org: Uuid,
    id: Uuid,
) -> Result<Option<UserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM scim_users WHERE org_id = $1 AND id = $2 FOR UPDATE"
    ))
    .bind(org)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?)
}

/// The global account behind a link, row-locked (`FOR UPDATE`) so a concurrent sign-in's
/// binding (identity insert + `email_verified_at`) and a SCIM e-mail change serialize.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkedAccount {
    pub email: String,
    /// An identity or a verified e-mail: a person signed in (or a verified bootstrap admin).
    /// SCIM never changes such an account (D-167(a), SEC-73).
    pub signed_in: bool,
}

pub async fn lock_account(
    conn: &mut PgConnection,
    user: Uuid,
) -> Result<Option<LinkedAccount>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.email::text AS email, \
                (u.email_verified_at IS NOT NULL \
                 OR EXISTS (SELECT 1 FROM user_identities i WHERE i.user_id = u.id)) AS signed_in \
         FROM users u WHERE u.id = $1 FOR UPDATE",
    )
    .bind(user)
    .fetch_optional(&mut *conn)
    .await?)
}

/// Does `user` belong to any org other than `org` — a membership or a SCIM link in any
/// state (tombstones included)? Then a placeholder is shared and its account is never
/// rewritten from this org (D-167(b)).
pub async fn user_elsewhere(
    conn: &mut PgConnection,
    org: Uuid,
    user: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM org_members WHERE user_id = $2 AND org_id <> $1) \
             OR EXISTS (SELECT 1 FROM scim_users WHERE user_id = $2 AND org_id <> $1)",
    )
    .bind(org)
    .bind(user)
    .fetch_one(&mut *conn)
    .await?)
}

/// A pre-provisioned global user (`04 §SCIM` Users step 2a): unverified e-mail, no identity.
/// The only `users` insert of the SCIM plane (POST, and an update moving a placeholder's
/// link, D-167(b)).
pub async fn insert_preprovisioned_user(
    conn: &mut PgConnection,
    id: Uuid,
    email: &str,
    display_name: &str,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2::citext, $3)")
        .bind(id)
        .bind(email)
        .bind(display_name)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Rename a still-unbound placeholder's e-mail in place (D-167(b)). Guarded in the
/// statement itself: never a user with a verified e-mail or an identity. Returns whether
/// the row was changed (`false` = it is no longer unbound; the caller refuses).
pub async fn set_placeholder_email(
    conn: &mut PgConnection,
    user: Uuid,
    email: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE users SET email = $2::citext, updated_at = now() \
         WHERE id = $1 AND email_verified_at IS NULL \
           AND NOT EXISTS (SELECT 1 FROM user_identities i WHERE i.user_id = users.id)",
    )
    .bind(user)
    .bind(email)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1)
}

/// Move a link to another global user (D-167(b) only: the link's user is a still-unbound
/// placeholder and the new e-mail belongs to another user). Every other update keeps the
/// link's `user_id` (D-167). Returns the row.
pub async fn move_user(
    conn: &mut PgConnection,
    org: Uuid,
    id: Uuid,
    user_id: Uuid,
) -> Result<Option<UserRow>, DbError> {
    sqlx::query(
        "UPDATE scim_users SET user_id = $3, updated_at = now() \
         WHERE org_id = $1 AND id = $2 AND state <> 'removed'",
    )
    .bind(org)
    .bind(id)
    .bind(user_id)
    .execute(&mut *conn)
    .await?;
    user_by_id(&mut *conn, org, id).await
}

/// Full update of the stored link attributes (`PUT`/`PATCH`); `state` changes go through
/// [`set_user_state`] so `suspended_by_scim` is decided there. The link's `user_id` is
/// immutable here (D-167; the one exception is [`move_user`]). Returns the row.
#[allow(clippy::too_many_arguments)]
pub async fn update_user(
    conn: &mut PgConnection,
    org: Uuid,
    id: Uuid,
    external_id: Option<&str>,
    user_name: &str,
    display_name: Option<&str>,
    given_name: Option<&str>,
    family_name: Option<&str>,
) -> Result<Option<UserRow>, DbError> {
    sqlx::query(
        "UPDATE scim_users SET external_id = $3, user_name = $4, \
             display_name = $5, given_name = $6, family_name = $7, updated_at = now() \
         WHERE org_id = $1 AND id = $2",
    )
    .bind(org)
    .bind(id)
    .bind(external_id)
    .bind(user_name)
    .bind(display_name)
    .bind(given_name)
    .bind(family_name)
    .execute(&mut *conn)
    .await?;
    user_by_id(&mut *conn, org, id).await
}

/// `active` ⇄ `suspended` on the link. `by_scim` records whether SCIM made the membership
/// `suspended` (only then may SCIM reactivate it, 04 §SCIM). A target state of `removed`
/// (the human offboarding reconciliation, FIX-037) tombstones the row like
/// [`remove_user`]: `removed_at` is stamped and the write refuses an already-`removed` row.
/// Returns the updated row.
pub async fn set_user_state(
    conn: &mut PgConnection,
    org: Uuid,
    id: Uuid,
    state: &str,
    by_scim: bool,
) -> Result<Option<UserRow>, DbError> {
    sqlx::query(
        "UPDATE scim_users SET state = $3, suspended_by_scim = $4, removed_at = \
             CASE WHEN $3 = 'removed' THEN now() ELSE NULL END, updated_at = now() \
         WHERE org_id = $1 AND id = $2 AND state <> 'removed'",
    )
    .bind(org)
    .bind(id)
    .bind(state)
    .bind(by_scim)
    .execute(&mut *conn)
    .await?;
    user_by_id(&mut *conn, org, id).await
}

/// Revive a tombstoned link (`POST /Users` of a removed user, C-474 "re-provisions"): the
/// row keeps its id and its pinned `user_id` (D-167: a revival never re-resolves the
/// e-mail, so it never moves the link either; `UNIQUE (org_id, user_id)` is never
/// re-inserted) and the attributes plus state are overwritten in place (`removed_at`
/// cleared). Guarded on `state = 'removed'` so only a genuine tombstone is revived;
/// returns the row, or `None` when the link is not a removed one.
#[allow(clippy::too_many_arguments)]
pub async fn revive_user(
    conn: &mut PgConnection,
    org: Uuid,
    id: Uuid,
    external_id: Option<&str>,
    user_name: &str,
    display_name: Option<&str>,
    given_name: Option<&str>,
    family_name: Option<&str>,
    state: &str,
    by_scim: bool,
) -> Result<Option<UserRow>, DbError> {
    sqlx::query(
        "UPDATE scim_users SET external_id = $3, user_name = $4, \
             display_name = $5, given_name = $6, family_name = $7, state = $8, \
             suspended_by_scim = $9, removed_at = NULL, updated_at = now() \
         WHERE org_id = $1 AND id = $2 AND state = 'removed'",
    )
    .bind(org)
    .bind(id)
    .bind(external_id)
    .bind(user_name)
    .bind(display_name)
    .bind(given_name)
    .bind(family_name)
    .bind(state)
    .bind(by_scim)
    .execute(&mut *conn)
    .await?;
    user_by_id(&mut *conn, org, id).await
}

/// Tombstone (`DELETE /Users/{id}`): the row stays so JIT and invitations cannot re-admit the
/// person into this org (04 §OIDC note). Returns the row *before* the change (the actor may
/// need the previous state), or `None` when unknown/foreign.
pub async fn remove_user(
    conn: &mut PgConnection,
    org: Uuid,
    id: Uuid,
) -> Result<Option<UserRow>, DbError> {
    let before = user_by_id(&mut *conn, org, id).await?;
    if before.is_some() {
        sqlx::query(
            "UPDATE scim_users SET state = 'removed', suspended_by_scim = false, \
                 removed_at = now(), updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND state <> 'removed'",
        )
        .bind(org)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(before)
}

// ---- groups ----------------------------------------------------------------------

/// The org's `organizations.settings` jsonb (the JIT/invitation gates read `scim.exclusive`
/// inside their transactions).
pub async fn settings_of<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<serde_json::Value, DbError> {
    Ok(
        sqlx::query_scalar("SELECT settings FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_one(exec)
            .await?,
    )
}

/// Number of the org's SCIM groups (list `totalResults`), optionally only those whose name
/// (case-insensitive) or `externalId` (exact) equals the filter value. No upper bound.
pub async fn scim_groups_count<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    display_name: Option<&str>,
    external_id: Option<&str>,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM groups g WHERE g.org_id = $1 AND g.source = 'scim'
           AND ($2::text IS NULL OR lower(g.name::text) = lower($2))
           AND ($3::text IS NULL OR g.external_id = $3)",
    )
    .bind(org)
    .bind(display_name)
    .bind(external_id)
    .fetch_one(exec)
    .await?)
}

/// One page of the org's SCIM groups (list), `created_at, id` order, `OFFSET`/`LIMIT` in SQL;
/// members are joined separately (16 §W15).
pub async fn scim_groups_window<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    display_name: Option<&str>,
    external_id: Option<&str>,
    offset: i64,
    limit: i64,
) -> Result<Vec<super::groups::GroupRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT g.id, g.org_id, g.name::text AS name, g.description, g.source, g.idp_provider, \
                g.idp_value, g.external_id, g.org_role, g.revision, g.created_by, g.created_at, \
                g.updated_at, \
                (SELECT count(*)::bigint FROM group_members m WHERE m.group_id = g.id \
                   AND m.org_id = g.org_id) AS member_count, \
                (SELECT count(*)::bigint FROM group_workspace_access b WHERE b.group_id = g.id \
                   AND b.org_id = g.org_id) AS workspace_count \
         FROM groups g WHERE g.org_id = $1 AND g.source = 'scim'
           AND ($2::text IS NULL OR lower(g.name::text) = lower($2))
           AND ($3::text IS NULL OR g.external_id = $3)
         ORDER BY g.created_at, g.id OFFSET $4 LIMIT $5",
    )
    .bind(org)
    .bind(display_name)
    .bind(external_id)
    .bind(offset)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

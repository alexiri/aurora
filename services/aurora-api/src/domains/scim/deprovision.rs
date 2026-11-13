//! SCIM deprovisioning effects (D-135, SEC-73; docs/04 §SCIM Deprovisioning): the full
//! matrix for `suspend` and `remove`, in one transaction with the caller's audit rows.
//!
//! | Effect | Suspend | Remove |
//! |--------|---------|--------|
//! | Org membership | `suspended` (role kept) | row deleted (bindings and group rows go with it) |
//! | Sessions | revoked only with **no other active membership** | same |
//! | Org API keys of the user | revoked (never revived) | same |
//! | Speaker turns of this org | queued `aborted`; live `turn.abort` + `aborted` `user_deprovisioned` | same |
//! | Pending invitations | cancelled | cancelled |
//! | Invalidation | `aurora_auth_revoked user` + org-scope `access` | same |

use crate::audit::{AuditActor, AuditEvent, Outcome};
use crate::domains::conversations::service as conv;
use crate::domains::hosted::lifecycle;
use aurora_core::ApiError;
use aurora_db::repo::{groups, members, sessions};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

/// Notice appended to conversations whose turn a deprovisioning ended.
const ABORT_NOTICE: &str =
    "This turn was stopped: the person was deactivated in the organization by the directory.";

/// Bound on the turns ended per round (the loop repeats until none remain; the freeze's
/// pattern).
const ABORT_ROUND: i64 = 500;

/// Suspend the membership: `org_members.status = suspended`, role kept.
pub async fn suspend_membership(
    conn: &mut PgConnection,
    org: Uuid,
    user: Uuid,
    actor: &AuditActor,
) -> Result<(), ApiError> {
    let cur = members::get(&mut *conn, org, user)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    if cur.status == "suspended" {
        return Ok(()); // idempotent
    }
    members::update(&mut *conn, org, user, None, Some("suspended")).await?;
    notify(conn, org, user).await?;
    let ev = AuditEvent::new(actor.clone(), "member.update", Outcome::Success)
        .org(org)
        .target("user", user)
        .metadata(json!({
            "via": crate::domains::scim::service::VIA,
            "change": {"status": format!("{} → suspended", cur.status)},
        }));
    crate::audit::insert_tx(&mut *conn, &ev)
        .await
        .map_err(|_| ApiError::unavailable("audit_unavailable", "The audit log is unavailable."))?;
    Ok(())
}

/// Reactivate a membership SCIM suspended (only allowed when `suspended_by_scim`; the caller
/// checks the flag).
pub async fn reactivate_membership(
    conn: &mut PgConnection,
    org: Uuid,
    user: Uuid,
    actor: &AuditActor,
) -> Result<(), ApiError> {
    members::update(&mut *conn, org, user, None, Some("active")).await?;
    notify(conn, org, user).await?;
    let ev = AuditEvent::new(actor.clone(), "member.update", Outcome::Success)
        .org(org)
        .target("user", user)
        .metadata(json!({
            "via": crate::domains::scim::service::VIA,
            "change": {"status": "suspended → active"},
        }));
    crate::audit::insert_tx(&mut *conn, &ev)
        .await
        .map_err(|_| ApiError::unavailable("audit_unavailable", "The audit log is unavailable."))?;
    Ok(())
}

/// Remove the membership row (workspace bindings and group memberships go with it, by the
/// composite foreign keys).
pub async fn remove_membership(
    conn: &mut PgConnection,
    org: Uuid,
    user: Uuid,
    actor: &AuditActor,
) -> Result<(), ApiError> {
    if members::get(&mut *conn, org, user).await?.is_none() {
        return Ok(()); // idempotent
    }
    members::remove(&mut *conn, org, user).await?;
    notify(conn, org, user).await?;
    let ev = AuditEvent::new(actor.clone(), "member.remove", Outcome::Success)
        .org(org)
        .target("user", user)
        .metadata(json!({"via": crate::domains::scim::service::VIA}));
    crate::audit::insert_tx(&mut *conn, &ev)
        .await
        .map_err(|_| ApiError::unavailable("audit_unavailable", "The audit log is unavailable."))?;
    Ok(())
}

/// Org API keys of the user (never of other orgs; never revived by a reactivation).
pub async fn revoke_org_keys(
    conn: &mut PgConnection,
    org: Uuid,
    user: Uuid,
) -> Result<u64, ApiError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE api_keys SET revoked_at = now(), updated_at = now()
         WHERE user_id = $2 AND org_id = $1 AND revoked_at IS NULL
         RETURNING id",
    )
    .bind(org)
    .bind(user)
    .fetch_all(&mut *conn)
    .await
    .map_err(aurora_db::DbError::from)?;
    for id in &ids {
        aurora_db::repo::notify_auth_revoked(&mut *conn, "key", *id).await?;
    }
    Ok(ids.len() as u64)
}

/// Sessions are revoked only when the user has no other active membership in any org
/// (`04 §SCIM`; otherwise they stay signed in to the other orgs). Returns the count.
pub async fn revoke_sessions_if_last(conn: &mut PgConnection, user: Uuid) -> Result<u64, ApiError> {
    let other: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM org_members
         WHERE user_id = $1 AND status = 'active')",
    )
    .bind(user)
    .fetch_one(&mut *conn)
    .await
    .map_err(aurora_db::DbError::from)?;
    if other {
        return Ok(0);
    }
    sessions::revoke_all_for_user_except_tx(&mut *conn, user, None)
        .await
        .map_err(ApiError::from)
}

/// Pending invitations of **this org** for the person are cancelled: addressed to the link's
/// `userName` or to the account's `users.email` — the two differ once the IdP renamed a
/// signed-in person (D-167(a)). Another org's invitations are never touched (SEC-73).
/// Returns the cancelled ids with their org.
pub async fn cancel_invitations(
    conn: &mut PgConnection,
    org: Uuid,
    user_name: &str,
    user: Uuid,
) -> Result<Vec<(Uuid, Uuid)>, ApiError> {
    Ok(sqlx::query_as(
        "UPDATE invitations SET status = 'cancelled', updated_at = now()
         WHERE org_id = $1 AND status = 'pending'
           AND (email = $2::citext OR email = (SELECT email FROM users WHERE id = $3))
         RETURNING id, org_id",
    )
    .bind(org)
    .bind(user_name)
    .bind(user)
    .fetch_all(&mut *conn)
    .await
    .map_err(aurora_db::DbError::from)?)
}

/// The speaker turns of `user` in `org`: queued ones `aborted`, live ones `turn.abort` then
/// `aborted` with `error_code = user_deprovisioned` (docs/10 line 226). Returns the
/// `(conversation, turn)` pairs whose buffered deltas must be flushed after commit.
pub async fn abort_turns(
    conn: &mut PgConnection,
    org: Uuid,
    user: Uuid,
) -> Result<Vec<(Uuid, Uuid)>, ApiError> {
    let mut flush = Vec::new();
    // Ending a turn promotes its `joining` follow-ups into queued turns; repeat until none
    // remain (the freeze's pattern).
    for _ in 0..50 {
        let turns = active_speaker_turns(conn, org, user).await?;
        if turns.is_empty() {
            break;
        }
        let mut ended = 0;
        for (turn, conversation) in turns {
            aurora_db::repo::conversations::lock(&mut *conn, conversation).await?;
            conv::append_event(
                conn,
                conversation,
                Some(turn),
                "notice",
                &json!({"code": "user_deprovisioned", "text": ABORT_NOTICE}),
            )
            .await?;
            if lifecycle::finish(
                conn,
                turn,
                "aborted",
                Some("user_deprovisioned"),
                Some("The speaker was deactivated in this organization."),
                None,
            )
            .await?
            .is_some()
            {
                ended += 1;
                flush.push((conversation, turn));
            }
        }
        if ended == 0 {
            break;
        }
    }
    Ok(flush)
}

/// Active speaker turns of `user` in the org (conversations reach the org through their
/// workspace; speaker turns only, `joining` follow-ups are handled by [`abort_turns`]'s
/// promotion loop).
async fn active_speaker_turns(
    conn: &mut PgConnection,
    org: Uuid,
    user: Uuid,
) -> Result<Vec<(Uuid, Uuid)>, ApiError> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.conversation_id FROM turns t
         JOIN conversations c ON c.id = t.conversation_id
         JOIN workspaces w ON w.id = c.workspace_id
         WHERE w.org_id = $1 AND t.speaker_user_id = $2
           AND t.role = 'turn' AND t.status IN ('queued','dispatched','running')
         ORDER BY t.created_at LIMIT $3",
    )
    .bind(org)
    .bind(user)
    .bind(ABORT_ROUND)
    .fetch_all(&mut *conn)
    .await
    .map_err(aurora_db::DbError::from)?)
}

/// Invalidation of a membership change: gateway identity cache (scope `user`), open streams
/// (`access`), and the auth-revoked NOTIFY that re-authorizes long-polls.
async fn notify(conn: &mut PgConnection, org: Uuid, user: Uuid) -> Result<(), ApiError> {
    groups::notify_member_changed(&mut *conn, org, user).await?;
    aurora_db::repo::notify_auth_revoked(&mut *conn, "user", user)
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

/// BYO agents the person created in the org (the audit metadata and the offboarding list).
pub async fn byo_agents_created(
    conn: &mut PgConnection,
    org: Uuid,
    user: Uuid,
) -> Result<Vec<(Uuid, String)>, ApiError> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.name FROM agents a
         JOIN workspaces w ON w.id = a.workspace_id
         WHERE w.org_id = $1 AND a.created_by = $2 ORDER BY a.created_at LIMIT 50",
    )
    .bind(org)
    .bind(user)
    .fetch_all(&mut *conn)
    .await
    .map_err(aurora_db::DbError::from)?)
}

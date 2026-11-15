//! `pending` memberships (D-175, FIX-082, SEC-73): SCIM *prepares* a membership for an existing
//! account (`04 §SCIM` Users step 3); only that person's own next verified OIDC sign-in makes
//! it `active` ([`activate_on_sign_in`]), and `active: false` / `DELETE` before that remove it
//! ([`remove`], nothing to deprovision: a pending member has no session, key, turn or access
//! of this org). A pending membership counts against the seat cap, can be placed in SCIM
//! groups, and is a non-member for every authorization check (the effective views, keys,
//! grants and the gateway filter `status = 'active'`).

use super::gates;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::ApiError;
use aurora_db::repo::users::UserRow;
use aurora_db::repo::{groups, members, notify_auth_revoked};
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// Remove the pending membership of `user` in `org`: the row goes (a group membership with
/// it, by the composite foreign key), a critical `member.remove` row is written (`via: scim`,
/// `was: pending`), and the person's pending invitations of the org are cancelled like for a
/// deprovisioned member. Sessions, keys and turns are deliberately untouched: the person never
/// had any access through this org, and their sessions belong to their other orgs.
pub async fn remove(
    conn: &mut PgConnection,
    org: Uuid,
    user: Uuid,
    user_name: &str,
    actor: &AuditActor,
) -> Result<(), ApiError> {
    members::remove(&mut *conn, org, user).await?;
    groups::notify_member_changed(&mut *conn, org, user).await?;
    super::deprovision::cancel_invitations(&mut *conn, org, user_name, user).await?;
    let ev = AuditEvent::new(actor.clone(), "member.remove", Outcome::Success)
        .org(org)
        .target("user", user)
        .metadata(json!({"via": super::service::VIA, "was": "pending"}));
    audit::insert_tx(&mut *conn, &ev)
        .await
        .map_err(|_| audit::unavailable())?;
    Ok(())
}

/// Activate every `pending` membership of `user` in the sign-in transaction (D-175(b)): called
/// after the account is resolved (and found enabled) and before the role and group sync, so
/// both see the person as the member they now are. Per org, in id order: the org lock (the one
/// every SCIM writer takes), the row re-read under it, the SCIM admission gate of FIX-080 (a
/// link that was suspended or removed meanwhile activates nothing and is audited as a denied
/// `member.add`, `path = pending_activation`, on its own connection), then `pending` →
/// `active` with a critical `member.add` row (`via: scim`, `was: pending`) and the usual gateway
/// identity / auth invalidation. Returns the orgs activated.
pub async fn activate_on_sign_in(
    tx: &mut PgConnection,
    pool: &PgPool,
    account: &UserRow,
) -> Result<Vec<Uuid>, ApiError> {
    let user = account.id;
    let mut activated = Vec::new();
    for org in members::pending_orgs(&mut *tx, user).await? {
        groups::lock_org(&mut *tx, org).await?;
        // Re-read under the lock: SCIM may have removed or changed the row meanwhile.
        let still = members::get(&mut *tx, org, user)
            .await?
            .is_some_and(|m| m.status == "pending");
        if !still {
            continue;
        }
        if let Some(reason) = gates::admission_gate(&mut *tx, org, user).await?.reason() {
            gates::audit_denied_detached(pool, org, user, "pending_activation", reason).await?;
            continue;
        }
        if !members::activate_pending(&mut *tx, org, user).await? {
            continue;
        }
        groups::notify_member_changed(&mut *tx, org, user).await?;
        notify_auth_revoked(&mut *tx, "user", user).await?;
        // The actor is the person: their own verified sign-in is what grants the membership.
        let ev = AuditEvent::new(
            AuditActor {
                kind: "user",
                id: Some(user),
                label: account.email.clone(),
                source: "dashboard",
                ip: None,
            },
            "member.add",
            Outcome::Success,
        )
        .org(org)
        .target("user", user)
        .metadata(json!({
            "via": super::service::VIA,
            "role": "member",
            "status": "active",
            "was": "pending",
        }));
        audit::insert_tx(&mut *tx, &ev)
            .await
            .map_err(|_| audit::unavailable())?;
        activated.push(org);
    }
    Ok(activated)
}

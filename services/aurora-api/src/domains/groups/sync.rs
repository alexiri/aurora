//! IdP-linked groups: the sign-in sync (P4-GRP-1, D-156, SEC-71).
//!
//! Runs in the sign-in transaction with the **validated** claim values of one provider. It sets
//! the user's memberships in that provider's IdP-linked groups to exactly the groups whose claim
//! value is present, only in orgs the user is an active member of: it never creates, reactivates
//! or removes an org membership, never touches manual/SCIM rows or another provider's rows, and
//! stores nothing for values that link no group. An absent claim removes every membership.
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_config::OidcProvider;
use aurora_db::DbError;
use aurora_db::repo::groups as repo;
use serde_json::json;
use uuid::Uuid;

fn actor(p: &OidcProvider) -> AuditActor {
    AuditActor {
        kind: "service",
        id: None,
        label: format!("idp:{}", p.id),
        source: "system",
        ip: None,
    }
}

/// The IdP-linked groups (`linked`: provider id and group count) whose provider does not sync
/// them any more — it is no longer configured, or has no `group_claim` — so nobody can ever be
/// added to them (FIX-083). The API's startup logs these by provider id and count only, never a
/// group name or claim value.
pub fn orphaned_providers(
    linked: &[(String, i64)],
    providers_with_group_claim: &[String],
) -> Vec<(String, i64)> {
    linked
        .iter()
        .filter(|(id, _)| !providers_with_group_claim.contains(id))
        .cloned()
        .collect()
}

/// An org membership left `active` (suspended by an admin or by SCIM): delete the person's
/// IdP-linked group rows in that org in the same transaction, audit each as a critical
/// `group_member.remove` by `actor` and invalidate the gateway identity (FIX-083, SEC-71).
/// Access the IdP grants must not survive a suspension — and so cannot come back with a
/// reactivation that no sign-in has re-synced; the next sign-in adds back what the claim says.
pub async fn drop_idp_memberships(
    tx: &mut sqlx::PgConnection,
    actor: &AuditActor,
    org: Uuid,
    user: Uuid,
    reason: &str,
) -> Result<(), DbError> {
    // The org lock first (the lock a sign-in sync and every SCIM writer take): a sync that
    // read the membership as active has committed its rows before this deletes them.
    repo::lock_org(&mut *tx, org).await?;
    let removed = repo::delete_idp_rows_of_member(&mut *tx, org, user).await?;
    if removed.is_empty() {
        return Ok(());
    }
    repo::notify_member_changed(&mut *tx, org, user).await?;
    for (group_id, provider, claim_value) in removed {
        let ev = AuditEvent::new(actor.clone(), "group_member.remove", Outcome::Success)
            .org(org)
            .target("group", group_id)
            .metadata(json!({
                "user_id": user, "via": "idp", "provider": provider,
                "claim_value": claim_value, "reason": reason,
            }));
        audit::insert_tx(&mut *tx, &ev)
            .await
            .map_err(|e| DbError::Other(sqlx::Error::Protocol(e.to_string())))?;
    }
    Ok(())
}

/// Startup warning (FIX-083): log every provider id that IdP-linked groups name but that no
/// longer syncs them. Provider ids and counts only. A failure is logged and never stops startup.
pub async fn warn_orphaned_providers(pool: &sqlx::PgPool, providers: &[OidcProvider]) {
    let linked = match repo::idp_linked_providers(pool).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "IdP-linked group check skipped");
            return;
        }
    };
    let with_claim: Vec<String> = providers
        .iter()
        .filter(|p| p.group_claim.is_some())
        .map(|p| p.id.clone())
        .collect();
    for (provider, groups) in orphaned_providers(&linked, &with_claim) {
        tracing::warn!(
            provider,
            groups,
            "IdP-linked groups name a sign-in provider that is not configured with a group_claim: nobody can be added to them"
        );
    }
}

pub async fn sync_signin(
    tx: &mut sqlx::PgConnection,
    p: &OidcProvider,
    user: Uuid,
    values: &[String],
) -> Result<(), DbError> {
    let outcome = repo::sync_idp_memberships_report(&mut *tx, user, &p.id, values).await?;
    // `GROUPS_PER_USER` (docs/04 §Groups limits): the excess is skipped, never half-applied.
    // No new audit kind: a warn log naming the org and counts, not the group names.
    for (org_id, skipped) in &outcome.cap_skipped {
        tracing::warn!(
            user_id = %user, org_id = %org_id, provider = %p.id, skipped,
            limit = repo::GROUPS_PER_USER,
            "IdP group sync skipped memberships: user is at the per-user group limit"
        );
    }
    for ch in outcome.changes {
        repo::notify_member_changed(&mut *tx, ch.org_id, user).await?;
        let action = if ch.added {
            "group_member.add"
        } else {
            "group_member.remove"
        };
        let ev = AuditEvent::new(actor(p), action, Outcome::Success)
            .org(ch.org_id)
            .target("group", ch.group_id)
            .metadata(json!({
                "user_id": user, "via": "idp", "provider": p.id, "claim_value": ch.idp_value,
            }));
        audit::insert_tx(&mut *tx, &ev)
            .await
            .map_err(|e| DbError::Other(sqlx::Error::Protocol(e.to_string())))?;
    }
    Ok(())
}

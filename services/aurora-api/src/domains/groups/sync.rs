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

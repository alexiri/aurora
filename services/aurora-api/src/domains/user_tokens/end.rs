//! Every end of a person's standing (D-284(a), C-855, C-858, SEC-113): one function per trigger,
//! each running inside the **trigger's own transaction**. A token that ends is marked `revoked`
//! (or `invalid` after a platform `401`), its `token_ref` is cleared, its pending `send_as_me`
//! approvals are rejected `credential_revoked`, its open sends lose their per-op copy (a send not
//! yet claimed is failed `credential_revoked`; a claimed one is already at the connector), and the
//! `user_token.revoke` audit row (security-critical) is written in the same transaction.
//!
//! The functions return the sealed values that left the database ([`Discards`]); the caller runs
//! [`Discards::run`] after its commit. A grant that ends (removed, or its agent deleted) ends only
//! that agent's standing: its pending approvals and its open sends, not the token.

use super::custody::Discards;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::ApiError;
use aurora_db::repo::user_tokens as repo;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

fn audit_failed(e: aurora_db::DbError) -> ApiError {
    tracing::error!(error = %e, "user token audit write failed");
    audit::unavailable()
}

/// End every token in `ids` as `revoked`. `reason` is one of `owner`, `link_removed`,
/// `owner_deleted`, `scim_deprovision`, `integration_deleted`, `org_deleted`.
pub async fn revoke(
    tx: &mut PgConnection,
    ids: &[Uuid],
    actor: &AuditActor,
    reason: &str,
) -> Result<Discards, ApiError> {
    let mut discards = Discards::default();
    for id in ids {
        let Some(ended) = repo::end_token(tx, *id, "revoked").await? else {
            continue;
        };
        discards.token(ended.token_id, ended.org_id, ended.token_ref.clone());
        let approvals = repo::cancel_pending_sends(tx, *id, None).await?;
        let copies = repo::strip_open_copies(tx, *id, None).await?;
        let cancelled_sends = copies.iter().filter(|c| c.failed).count();
        for c in copies {
            discards.copy(c.op_id, c.org_id, c.sealed);
        }
        let ev = AuditEvent::new(actor.clone(), "user_token.revoke", Outcome::Success)
            .org(ended.org_id)
            .target("user_token", id)
            .metadata(json!({
                "reason": reason,
                "integration_id": ended.integration_id,
                "cancelled_approvals": approvals.len(),
                "cancelled_sends": cancelled_sends,
            }));
        audit::insert_tx(tx, &ev).await.map_err(audit_failed)?;
    }
    Ok(discards)
}

/// A platform `401` on a verify or a send: the token becomes `invalid` (its sealed value is
/// discarded, pending approvals rejected). The presence is untouched.
pub async fn invalidate(tx: &mut PgConnection, id: Uuid) -> Result<Discards, ApiError> {
    let mut discards = Discards::default();
    let Some(ended) = repo::end_token(tx, id, "invalid").await? else {
        return Ok(discards);
    };
    discards.token(ended.token_id, ended.org_id, ended.token_ref.clone());
    let approvals = repo::cancel_pending_sends(tx, id, None).await?;
    let copies = repo::strip_open_copies(tx, id, None).await?;
    for c in copies {
        discards.copy(c.op_id, c.org_id, c.sealed);
    }
    let ev = AuditEvent::new(
        AuditActor::system("user-token"),
        "user_token.invalid",
        Outcome::Success,
    )
    .org(ended.org_id)
    .target("user_token", id)
    .metadata(json!({
        "reason": "platform_rejected",
        "integration_id": ended.integration_id,
        "cancelled_approvals": approvals.len(),
    }));
    audit::insert_tx(tx, &ev).await.map_err(audit_failed)?;
    Ok(discards)
}

/// One grant ends (the owner removed it, or the agent is going away): the agent's pending
/// approvals are rejected `credential_revoked` and its open sends lose their copy.
pub async fn end_grant(
    tx: &mut PgConnection,
    token: Uuid,
    agent: Uuid,
) -> Result<Discards, ApiError> {
    let mut discards = Discards::default();
    repo::cancel_pending_sends(tx, token, Some(agent)).await?;
    for c in repo::strip_open_copies(tx, token, Some(agent)).await? {
        discards.copy(c.op_id, c.org_id, c.sealed);
    }
    Ok(discards)
}

/// Deletion of an agent: every grant it holds ends (the rows cascade with the agent).
pub async fn for_agent(
    tx: &mut PgConnection,
    agent: Uuid,
    actor: &AuditActor,
) -> Result<Discards, ApiError> {
    let tokens: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT g.user_token_id, i.org_id FROM user_token_agents g
           JOIN user_tokens t ON t.id = g.user_token_id
           JOIN channel_integrations i ON i.id = t.integration_id
          WHERE g.agent_id = $1 FOR UPDATE OF t",
    )
    .bind(agent)
    .fetch_all(&mut *tx)
    .await
    .map_err(aurora_db::DbError::from)?;
    let mut discards = Discards::default();
    for (token, org) in tokens {
        discards.extend(end_grant(tx, token, agent).await?);
        let ev = AuditEvent::new(actor.clone(), "user_token.ungrant", Outcome::Success)
            .org(org)
            .target("user_token", token)
            .metadata(json!({"agent_id": agent, "reason": "agent_deleted"}));
        audit::insert_tx(tx, &ev).await.map_err(audit_failed)?;
    }
    Ok(discards)
}

/// Deletion of a workspace: every grant its agents hold ends with them.
pub async fn for_workspace(
    tx: &mut PgConnection,
    workspace: Uuid,
    actor: &AuditActor,
) -> Result<Discards, ApiError> {
    let agents: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT g.agent_id FROM user_token_agents g
           JOIN agents a ON a.id = g.agent_id WHERE a.workspace_id = $1",
    )
    .bind(workspace)
    .fetch_all(&mut *tx)
    .await
    .map_err(aurora_db::DbError::from)?;
    let mut discards = Discards::default();
    for agent in agents {
        discards.extend(for_agent(tx, agent, actor).await?);
    }
    Ok(discards)
}

/// The owner is deleted, disabled out of the directory (SCIM) or otherwise gone.
pub async fn for_user(
    tx: &mut PgConnection,
    user: Uuid,
    actor: &AuditActor,
    reason: &str,
) -> Result<Discards, ApiError> {
    let ids = repo::live_ids_for_user(tx, user).await?;
    revoke(tx, &ids, actor, reason).await
}

/// The integration (or its whole organization) is deleted.
pub async fn for_integration(
    tx: &mut PgConnection,
    integration: Uuid,
    actor: &AuditActor,
) -> Result<Discards, ApiError> {
    let ids = repo::live_ids_for_integration(tx, integration).await?;
    revoke(tx, &ids, actor, "integration_deleted").await
}

pub async fn for_org(
    tx: &mut PgConnection,
    org: Uuid,
    actor: &AuditActor,
) -> Result<Discards, ApiError> {
    let ids = repo::live_ids_for_org(tx, org).await?;
    revoke(tx, &ids, actor, "org_deleted").await
}

/// The person's link on `integration` is going away: tokens bound to that account end.
pub async fn for_link(
    tx: &mut PgConnection,
    user: Uuid,
    integration: Uuid,
    actor: &AuditActor,
) -> Result<Discards, ApiError> {
    let ids = repo::live_ids_for_link(tx, user, integration).await?;
    revoke(tx, &ids, actor, "link_removed").await
}

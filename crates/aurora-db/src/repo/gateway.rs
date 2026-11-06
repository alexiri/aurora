//! Gateway reads/writes: credential identity lookup and instance bookkeeping
//! (docs/06 §Credential authentication, docs/03 §gateway_instances).

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CredentialIdentityRow {
    pub credential_id: Uuid,
    pub kind: String,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub org_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub context_label: Option<String>,
    /// Seconds until `expires_at` (None = no expiry).
    pub expires_in_secs: Option<f64>,
}

/// Live credential by SHA-256 hash: not revoked, not expired, agent/workspace/org
/// present, and the org has at least one active owner.
pub async fn find_credential_identity<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<Option<CredentialIdentityRow>, DbError> {
    Ok(sqlx::query_as::<_, CredentialIdentityRow>(
        "SELECT c.id AS credential_id, c.kind, c.agent_id, a.workspace_id, w.org_id, \
                c.conversation_id, c.turn_id, c.context_label, \
                EXTRACT(EPOCH FROM (c.expires_at - now()))::float8 AS expires_in_secs \
         FROM gateway_credentials c \
         JOIN agents a ON a.id = c.agent_id \
         JOIN workspaces w ON w.id = a.workspace_id \
         JOIN organizations o ON o.id = w.org_id \
         WHERE c.token_hash = $1 \
           AND c.revoked_at IS NULL \
           AND (c.expires_at IS NULL OR c.expires_at > now()) \
           AND EXISTS (SELECT 1 FROM org_members m \
                       WHERE m.org_id = o.id AND m.role = 'owner' AND m.status = 'active')",
    )
    .bind(token_hash)
    .fetch_optional(exec)
    .await?)
}

/// Users with `can_use` on the workspace: active org members who are
/// owner/admin or hold `workspace_access`. Suspended members are excluded.
pub async fn principals<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace_id: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT m.user_id FROM workspaces w \
         JOIN org_members m ON m.org_id = w.org_id AND m.status = 'active' \
         WHERE w.id = $1 \
           AND (m.role IN ('owner', 'admin') \
                OR EXISTS (SELECT 1 FROM workspace_access x \
                           WHERE x.workspace_id = w.id AND x.user_id = m.user_id)) \
         ORDER BY m.user_id",
    )
    .bind(workspace_id)
    .fetch_all(exec)
    .await?)
}

pub async fn register_instance<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    hostname: &str,
    version: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at) \
         VALUES ($1, $2, $3, now(), now())",
    )
    .bind(id)
    .bind(hostname)
    .bind(version)
    .execute(exec)
    .await?;
    Ok(())
}

/// Churn column: does not touch `updated_at`.
pub async fn heartbeat_instance<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query("UPDATE gateway_instances SET last_seen_at = now() WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?;
    Ok(())
}

pub async fn stop_instance<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE gateway_instances SET stopped_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

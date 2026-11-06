//! Grant compilation (07 §Grants and their compilation).
use aurora_core::ApiError;
use aurora_db::repo::derived_rules;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

/// Compile a secret grant: one `allow` rule on `secret{id}` for the agent, in the agent's
/// workspace scope. Runs in the caller's transaction. BE-033 owns the full compiler.
pub async fn compile_secret_grant(
    conn: &mut PgConnection,
    org_id: Uuid,
    workspace_id: Uuid,
    grant_id: Uuid,
    agent_id: Uuid,
    secret_id: Uuid,
    secret_name: &str,
) -> Result<(), ApiError> {
    let rule = json!({
        "name": format!("Grant: {secret_name}"),
        "enabled": true,
        "identities": [{"kind": "agent", "id": agent_id}],
        "targets": [{"type": "secret", "id": secret_id}],
        "action": "allow",
        "require_approval": false,
        "rate_limit": null,
        "conditions": [],
    });
    derived_rules::upsert(
        &mut *conn,
        org_id,
        Some(workspace_id),
        "grant",
        grant_id,
        0,
        &rule,
    )
    .await?;
    Ok(())
}

//! Grant compilation (07 §Grants and their compilation).
use super::dto::GrantMode;
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

fn conn_rule(
    name: String,
    agent_id: Uuid,
    connection_id: Uuid,
    tools: Option<&[String]>,
    action: &str,
    require_approval: bool,
) -> serde_json::Value {
    let mut target = json!({"type": "connection", "id": connection_id});
    if let Some(t) = tools {
        target["tools"] = json!(t);
    }
    json!({
        "name": name,
        "enabled": true,
        "identities": [{"kind": "agent", "id": agent_id}],
        "targets": [target],
        "action": action,
        "require_approval": require_approval,
        "rate_limit": null,
        "conditions": [],
    })
}

/// Compile a connection grant (07 table): `full` is one allow rule; `custom` is allow
/// (ordinal 0), allow+approval (1) and a final block of everything else (2). Stale
/// ordinals from a previous mode are removed. Runs in the caller's transaction.
#[allow(clippy::too_many_arguments)]
pub async fn compile_connection_grant(
    conn: &mut PgConnection,
    org_id: Uuid,
    workspace_id: Uuid,
    grant_id: Uuid,
    agent_id: Uuid,
    connection_id: Uuid,
    label: &str,
    mode: GrantMode,
    allow_tools: &[String],
    ask_tools: &[String],
) -> Result<(), ApiError> {
    let name = format!("Grant: {label}");
    let mut rules: Vec<(i32, serde_json::Value)> = Vec::new();
    match mode {
        GrantMode::Full => rules.push((
            0,
            conn_rule(name, agent_id, connection_id, None, "allow", false),
        )),
        GrantMode::Custom => {
            if !allow_tools.is_empty() {
                rules.push((
                    0,
                    conn_rule(
                        name.clone(),
                        agent_id,
                        connection_id,
                        Some(allow_tools),
                        "allow",
                        false,
                    ),
                ));
            }
            if !ask_tools.is_empty() {
                rules.push((
                    1,
                    conn_rule(
                        format!("{name} (ask)"),
                        agent_id,
                        connection_id,
                        Some(ask_tools),
                        "allow",
                        true,
                    ),
                ));
            }
            rules.push((
                2,
                conn_rule(
                    format!("{name} (block rest)"),
                    agent_id,
                    connection_id,
                    None,
                    "block",
                    false,
                ),
            ));
        }
    }
    let keep: Vec<i32> = rules.iter().map(|(o, _)| *o).collect();
    derived_rules::delete_except(&mut *conn, "grant", grant_id, &keep).await?;
    for (ordinal, rule) in &rules {
        derived_rules::upsert(
            &mut *conn,
            org_id,
            Some(workspace_id),
            "grant",
            grant_id,
            *ordinal,
            rule,
        )
        .await?;
    }
    Ok(())
}

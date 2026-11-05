//! Least-privilege grants for the gateway database role (03 §Gateway database role grants).
//!
//! The role name is configuration (`database.gateway_role`), so this is not a static migration.
//! Run [`apply_gateway_grants`] after every `migrate`; it is idempotent.

use sqlx::PgPool;

/// Failure applying the gateway grants.
#[derive(Debug, thiserror::Error)]
pub enum GrantsError {
    #[error("invalid gateway role name {0:?}: must match [a-z_][a-z0-9_]{{0,62}}")]
    InvalidRole(String),
    #[error("gateway role {0:?} does not exist; create it before applying grants")]
    RoleMissing(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// `[a-z_][a-z0-9_]{0,62}`
fn valid_role(role: &str) -> bool {
    let b = role.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// Double-quote an identifier (embedded quotes doubled).
fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Create the gateway views and grant `role` exactly the privileges listed in 03.
///
/// Runs in one transaction. Fails with [`GrantsError::RoleMissing`] if the role does not exist.
pub async fn apply_gateway_grants(pool: &PgPool, role: &str) -> Result<(), GrantsError> {
    if !valid_role(role) {
        return Err(GrantsError::InvalidRole(role.to_owned()));
    }
    let mut tx = pool.begin().await?;

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1)")
            .bind(role)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Err(GrantsError::RoleMissing(role.to_owned()));
    }

    let schema: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(&mut *tx)
        .await?;
    let s = quote_ident(&schema);
    let r = quote_ident(role);

    let mut stmts: Vec<String> = vec![
        // Column-restricted views (run with the owner's privileges; the role gets SELECT on the view only).
        format!(
            "CREATE OR REPLACE VIEW {s}.gateway_users AS SELECT id, email, disabled_at FROM {s}.users"
        ),
        format!(
            "CREATE OR REPLACE VIEW {s}.gateway_plugin_packages AS \
             SELECT id, package_id, version, digest, status, status_reason FROM {s}.plugin_packages"
        ),
        format!("GRANT USAGE ON SCHEMA {s} TO {r}"),
    ];

    const SELECT_TABLES: &[&str] = &[
        "organizations",
        "workspaces",
        "workspace_access",
        "org_members",
        "gateway_users",
        "api_keys",
        "agents",
        "gateway_credentials",
        "secrets",
        "app_connections",
        "app_oauth_clients",
        "grants",
        "derived_rules",
        "policy_generations",
        "policy_rules",
        "registry_snapshots",
        "gateway_plugin_packages",
        "host_claim_decisions",
        "org_app_availability",
    ];
    for t in SELECT_TABLES {
        stmts.push(format!("GRANT SELECT ON {s}.{t} TO {r}"));
    }
    stmts.push(format!(
        "GRANT INSERT, UPDATE ON {s}.approvals, {s}.gateway_instances TO {r}"
    ));
    stmts.push(format!("GRANT INSERT ON {s}.request_log TO {r}"));
    stmts.push(format!(
        "GRANT UPDATE (credentials_ref, credentials_version, credentials_expires_at, status, \
         status_reason, reauth_required_at, last_used_at) ON {s}.app_connections TO {r}"
    ));
    stmts.push(format!(
        "GRANT UPDATE (last_used_at, origin) ON {s}.secrets TO {r}"
    ));
    // pg_advisory_lock family (both key forms, session/xact, exclusive/shared).
    for f in [
        "pg_advisory_lock",
        "pg_advisory_lock_shared",
        "pg_try_advisory_lock",
        "pg_try_advisory_lock_shared",
        "pg_advisory_unlock",
        "pg_advisory_unlock_shared",
        "pg_advisory_xact_lock",
        "pg_advisory_xact_lock_shared",
        "pg_try_advisory_xact_lock",
        "pg_try_advisory_xact_lock_shared",
    ] {
        for args in ["bigint", "integer, integer"] {
            stmts.push(format!(
                "GRANT EXECUTE ON FUNCTION pg_catalog.{f}({args}) TO {r}"
            ));
        }
    }
    stmts.push(format!(
        "GRANT EXECUTE ON FUNCTION pg_catalog.pg_advisory_unlock_all() TO {r}"
    ));

    for sql in &stmts {
        sqlx::query(sql).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::valid_role;

    #[test]
    fn role_validation() {
        assert!(valid_role("aurora_gateway"));
        assert!(valid_role("_x1"));
        assert!(!valid_role(""));
        assert!(!valid_role("1abc"));
        assert!(!valid_role("Abc"));
        assert!(!valid_role("a-b"));
        assert!(!valid_role("a\"; DROP"));
        assert!(!valid_role(&"a".repeat(64)));
        assert!(valid_role(&"a".repeat(63)));
    }
}
